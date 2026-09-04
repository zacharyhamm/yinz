//! What an agent pane says about itself.
//!
//! A pane's agent record (`PaneInfo::agent`) is written by three roads: the spawn path, for a
//! pane a plugin opened as an agent pane; a plugin, through `PluginCommand::SetPaneAgent`; and
//! the agent's own hooks, through `zellij action agent-report --hook-json`. This module reads
//! the common Claude Code/Codex hook shape so the binary and the plugins that draw the result
//! cannot drift apart.

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
    // The turn ended: what moves the record back to `idle`. It also carries
    // `last_assistant_message`, which nothing buys a summary from any more - a summary is
    // bought once, on `SessionEnd`. See `docs/AGENT_HOOKS.md`.
    "Stop",
    // The turn ended in an API error. Carries the same message field, and is the only
    // thing that says so: without it a failed turn leaves the record on `busy` until the
    // user types again, which for a record that expires on nothing is the worst case
    // there is.
    "StopFailure",
    // The feed ends.
    "SessionEnd",
];

/// The Codex hook events used to report a pane's state.
///
/// Codex has no `Notification` hook. Its question goes up as an ordinary `PreToolUse` for
/// `request_user_input`, which is named in [`WAITING_TOOLS`] beside Claude Code's two and read
/// as the equivalent waiting signal.
pub const CODEX_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Interrupt",
    "Stop",
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

/// Codex caps `SessionEnd` hook timeouts at three seconds, so every reporting hook uses that
/// same short bound.
pub const CODEX_HOOK_TIMEOUT_SECONDS: u64 = 3;

/// The command a spawned Claude Code pane's hooks run, with no script in between: the pane
/// inherits `ZELLIJ_PANE_ID`, which the action reads itself, and `zellij` is on the path the
/// pane's shell found it on.
pub const CLAUDE_HOOK_REPORT_COMMAND: &str = "zellij action agent-report --hook-json";

/// The pipe the raw event is broadcast on beside the record, and the `--args` keys naming the
/// pane it fired in and the agent it came from.
///
/// `agent-report --hook-json` sets the pane's record *and* hands the event itself to whatever
/// plugin wants more than a record holds - hocus's daemon reads `last_assistant_message` off a
/// `SessionEnd` to know a run is over and worth summarising, and `permission_mode` off a
/// `PermissionRequest` to decide whether a wait is worth interrupting somebody over. Spelled here rather than in the plugin crate so
/// the two ends cannot drift: a pipe name that does not match is silent.
///
/// The kind travels because the payload cannot carry it: a hook event is the agent's own JSON
/// and says nothing about which agent wrote it, and the daemon writes a record of its own from
/// what arrives here. Absent - an old script that only pipes - it is a `claude`, which is what
/// every such script was installed for.
pub const AGENT_HOOK_PIPE: &str = "agent_hook";
pub const AGENT_HOOK_PANE_ID_ARG: &str = "pane_id";
pub const AGENT_HOOK_KIND_ARG: &str = "kind";

/// One command handler entry, as Claude Code and Codex spell it.
pub fn hook_handler(command: &str, timeout: u64) -> Value {
    json!({
        "type": "command",
        "command": command,
        "timeout": timeout,
    })
}

/// A Claude Code settings document whose `hooks` wire every event in [`CLAUDE_HOOK_EVENTS`]
/// to `command`. What `zellij hocus hooks install` merges into a settings file, and what a
/// spawned pane is handed whole as `--settings`; the two cannot disagree about the events
/// because this is the one place they are listed.
pub fn claude_hook_settings(command: &str) -> Value {
    hook_settings(CLAUDE_HOOK_EVENTS, command, CLAUDE_HOOK_TIMEOUT_SECONDS)
}

/// A Codex `hooks.json` document wiring every event in [`CODEX_HOOK_EVENTS`].
pub fn codex_hook_settings(command: &str) -> Value {
    hook_settings(CODEX_HOOK_EVENTS, command, CODEX_HOOK_TIMEOUT_SECONDS)
}

/// A hook settings document for one event list. Claude Code and Codex deliberately use the
/// same handler shape, so configuration generation needs no provider abstraction beyond this.
pub fn hook_settings(events: &[&str], command: &str, timeout: u64) -> Value {
    let hooks: serde_json::Map<String, Value> = events
        .iter()
        // No `matcher`: an entry without one fires on every occurrence of the event.
        .map(|event| {
            (
                (*event).to_owned(),
                json!([{ "hooks": [hook_handler(command, timeout)] }]),
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
///
/// **No notification means the turn *ended*, and `agent_completed` least of all.** It reads
/// like the one that would, and it is the only kind that names a finish — but Claude Code
/// emits it from the background-job monitor, for an async agent this session started, and the
/// payload carries the current session's fields rather than that job's. A pane whose Claude
/// launched a background agent and worked on would be marked idle mid-turn by it. The turn
/// ending has its own events, `Stop` and `StopFailure`, which arrive earlier and carry
/// `last_assistant_message` besides.
///
/// `agent_needs_input` is on this list despite coming from that same monitor, because it has a
/// second source that the finish notification does not: the foreground dialog map, which
/// raises it for teammate setup, a Slack-connect dialog and an offline file sync. Those are
/// this session, blocked, with a person to tell.
pub const WAITING_NOTIFICATIONS: &[&str] = &[
    "permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
    "worker_permission_prompt",
];

/// The tools whose `PreToolUse` *is* the question going up.
///
/// Every other tool starting is work, but these three do nothing except put a prompt in front
/// of the user and wait. Their `PostToolUse` is ordinary work again, which is what clears the
/// wait - so a tool named here needs no special case on the way out.
///
/// Read by tool name and not by agent, deliberately. `request_user_input` is Codex's, which has
/// no `AskUserQuestion` and no `Notification` either, but the list is what *every* road that
/// reads an event consults - the record `agent-report` writes, and `hocus_proto`'s `hook_phase`,
/// which the daemon draws its feed from. A mapping keyed on the agent's kind would reach only
/// the first of them, and the second would then overwrite it with `busy`.
pub const WAITING_TOOLS: &[&str] = &["AskUserQuestion", "ExitPlanMode", "request_user_input"];

/// The permission modes that answer a permission prompt without a person.
///
/// A permission prompt reported under one of these usually has nobody to interrupt: either the
/// mode decided already, or the prompt that fired is one the user has said in advance they do
/// not want to see. The state is still recorded - it is what the agent said - but see
/// [`waiting_needs_a_person`].
///
/// `bypassPermissions` and `dontAsk` do answer *every* one: Claude Code resolves them to a
/// blanket allow and a blanket deny. `auto` resolves to a classifier, which clears what it
/// judges safe and puts the rest in front of the user exactly as `default` would - so a wait
/// under it is silenced here on the odds, not on a guarantee. What recovers the ones the
/// classifier refused is the backstop: the `Notification` six seconds later, which by its own
/// existence proves nothing answered the prompt. The daemon holds the suppressed edge open for
/// it rather than counting the wait as already announced.
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

/// The largest hook event handed to the plugins exactly as it arrived.
///
/// A hook event is broadcast to *every* running plugin, which means a copy into each of some
/// fifteen wasm memories, twice per tool call, inside a hook with a five-second timeout. Two
/// fields have no bound at all in Claude Code's own schema — `tool_input`, so a `Write` of a
/// two-megabyte file, and `tool_response`, so a `Read` of anything — and neither is worth that
/// much of a turn. Over this, the event goes out through [`HookEvent::pruned`] instead of
/// verbatim; under it, nothing changes, which is nearly always.
pub const HOOK_BROADCAST_MAX_BYTES: usize = 64 * 1024;

/// The longest any one string in a pruned event's `tool_input` keeps, unless it sits under one
/// of [`PRUNED_HISTORY_KEYS`].
///
/// Comfortably above [`WAITING_DETAIL_CHARS`], which is the most a *record* takes from one of
/// them — a tool name and one argument's worth of hint. The keys a history reads at length are
/// the exception, and are cut to [`PRUNED_HISTORY_STRING_CHARS`] instead; this was their cap
/// too until a prune was found to be turning a submitted plan into 256 characters.
const PRUNED_STRING_CHARS: usize = 256;

/// The longest a pruned event keeps a `tool_input` string that becomes run history.
///
/// Matched to [`PRUNED_MESSAGE_CHARS`], because a plan and a final message are the same kind
/// of thing to the same reader: prose, kept whole, shown in a pane that wraps and scrolls.
const PRUNED_HISTORY_STRING_CHARS: usize = 8 * 1024;

/// What one pruned event's `tool_input` may spend in total, over which the remaining keys and
/// elements are dropped. Only a pathological object — thousands of small keys — reaches it,
/// because every string is cut first.
///
/// Comfortably above [`PRUNED_HISTORY_STRING_CHARS`], so the budget cannot quietly undo the
/// larger cap by running out inside the plan it was raised for.
const PRUNED_INPUT_BUDGET: usize = 32 * 1024;

/// The longest prose a pruned event keeps, per field. `last_assistant_message` is the one
/// genuinely used at length — it is kept on the run's timeline, and the run is what a summary
/// is made of — and its reader cuts it again to six thousand characters, so this is set above
/// that: a prune takes nothing the summary was going to keep.
const PRUNED_MESSAGE_CHARS: usize = 8 * 1024;

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
    /// The provider's identity for one tool call. Accept the alternate `tool_call_id` spelling
    /// so both hook formats reach one deduplication path.
    #[serde(default, alias = "tool_call_id")]
    pub tool_use_id: Option<String>,
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
    /// transcript file", and that is exactly what it buys here: a run's timeline keeps the
    /// agent's own words rather than a photograph of its terminal, and the summary made at
    /// the end of the session is made of those. Optional in Claude Code's schema, so a turn
    /// that ended without one contributes nothing.
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

impl HookEvent {
    /// This event, cut to a size worth broadcasting. See [`HOOK_BROADCAST_MAX_BYTES`].
    ///
    /// Every field survives — what is cut is how much of each one does. Strings first and a
    /// budget second, deliberately in that order: the keys a reader actually looks at inside
    /// `tool_input` are `command`, `file_path` and `questions[0]`, none of which is long, so
    /// cutting each string to [`PRUNED_STRING_CHARS`] leaves them all intact whatever order
    /// they arrived in. The budget only ever bites on an object with more keys than a tool
    /// call has any business carrying, and it drops the tail rather than the head.
    ///
    /// Anything Claude Code sends that this struct does not model — `tool_response` above
    /// all — is gone, because a pruned event travels re-serialized from the fields here.
    /// Nothing in this tree reads those: every reader parses this same struct.
    pub fn pruned(&self) -> HookEvent {
        self.pruned_to(PRUNED_INPUT_BUDGET)
    }

    /// [`pruned`](Self::pruned) with the budget named, so a payload that is still too big
    /// after a prune can be pruned harder rather than sent anyway. See [`broadcast_json`].
    ///
    /// `budget` bounds `tool_input` and every prose field alike: each is cut to its char cap
    /// and then to the budget in *bytes*, which is the unit the cap that matters is in. The
    /// two disagree by up to four times on text that is not ASCII, and a plan written in
    /// Japanese is exactly the case where the char cap alone let a "pruned" event out at
    /// several times the size a prune exists to enforce.
    ///
    /// A budget of zero leaves the metadata and nothing else, which is what makes
    /// [`broadcast_json`]'s descent terminate.
    pub fn pruned_to(&self, budget: usize) -> HookEvent {
        let cut = |text: &Option<String>, chars: usize| {
            text.as_deref()
                .map(|text| truncate_within(text, chars, budget))
        };
        HookEvent {
            hook_event_name: self.hook_event_name.clone(),
            session_id: self.session_id.clone(),
            source: self.source.clone(),
            cwd: self.cwd.clone(),
            notification_type: self.notification_type.clone(),
            tool_use_id: self.tool_use_id.clone(),
            tool_name: self.tool_name.clone(),
            tool_input: self.tool_input.as_ref().map(|input| {
                let mut budget = budget;
                prune_value(input, None, &mut budget)
            }),
            last_assistant_message: cut(&self.last_assistant_message, PRUNED_MESSAGE_CHARS),
            message: cut(&self.message, PRUNED_MESSAGE_CHARS),
            permission_mode: self.permission_mode.clone(),
            prompt: cut(&self.prompt, PRUNED_MESSAGE_CHARS),
            agent_id: self.agent_id.clone(),
        }
    }

    /// This event as the bytes to broadcast, at or under [`HOOK_BROADCAST_MAX_BYTES`].
    ///
    /// `json` is the event exactly as it arrived, which is what goes out whenever it fits —
    /// nearly always. Over the cap it is re-serialized from a prune, and the prune is repeated
    /// with a smaller budget until the result fits: a single cap on the input cannot promise
    /// an output size, because JSON escaping expands what it encodes — six bytes for one
    /// control character — and there are several fields drawing on the budget. The last step
    /// spends nothing, leaving the metadata alone, so the descent always ends under the cap.
    pub fn broadcast_json(&self, json: String) -> String {
        if json.len() <= HOOK_BROADCAST_MAX_BYTES {
            return json;
        }
        let mut budget = PRUNED_INPUT_BUDGET;
        loop {
            if let Ok(candidate) = serde_json::to_string(&self.pruned_to(budget)) {
                if candidate.len() <= HOOK_BROADCAST_MAX_BYTES || budget == 0 {
                    return candidate;
                }
            } else if budget == 0 {
                // Unreachable: this struct has no map keys and no non-finite numbers, so it
                // serializes. Returning the original is still better than looping.
                return json;
            }
            budget /= 8;
        }
    }
}

/// The first `chars` characters of `text` that also fit in `bytes`, on a character boundary.
///
/// Both units, because the caps are written in characters — a plan is prose, and prose is
/// counted in characters by everyone who reads it — while the budget that has to hold is a
/// byte count. One character is up to four bytes, so the two are not interchangeable on any
/// text that is not ASCII.
fn truncate_within(text: &str, chars: usize, bytes: usize) -> String {
    let end = text
        .char_indices()
        .take(chars)
        .map(|(start, ch)| start + ch.len_utf8())
        .take_while(|end| *end <= bytes)
        .last()
        .unwrap_or(0);
    text[..end].to_owned()
}

/// The `tool_input` keys whose text is prose somebody reads at length, rather than an argument
/// a row names in passing.
///
/// A plan and a question are the two things a hook event carries that become *history* — see
/// `run_entry` in hocus-proto — and the history has a wrapping, scrolling half-pane to show
/// them in. Everything else under `tool_input` is read for a word or two.
const PRUNED_HISTORY_KEYS: &[&str] = &["plan", "step", "question", "header"];

/// One `tool_input` value with every string cut and `budget` bytes to spend on the rest.
///
/// `key` is the object key this value sits under, because how much of a string is worth
/// keeping depends on who reads it: see [`PRUNED_HISTORY_KEYS`].
///
/// Recursion is bounded by the budget rather than by a depth count: a nested value costs its
/// parent's budget, so a deep structure runs out the same way a wide one does. An array's
/// elements inherit their array's key, so `steps[].step` and `questions[]` are read the same
/// whether the prose sits in the array or under a key inside each element.
fn prune_value(
    value: &serde_json::Value,
    key: Option<&str>,
    budget: &mut usize,
) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(text) => {
            let cap = match key.is_some_and(|key| PRUNED_HISTORY_KEYS.contains(&key)) {
                true => PRUNED_HISTORY_STRING_CHARS,
                false => PRUNED_STRING_CHARS,
            };
            // The budget as well as the cap, so the last string admitted cannot carry the
            // total past it — the check above only ever runs *before* a value.
            let cut = truncate_within(text, cap, *budget);
            *budget = budget.saturating_sub(cut.len());
            Value::String(cut)
        },
        Value::Array(items) => {
            let mut kept = Vec::new();
            for item in items {
                if *budget == 0 {
                    break;
                }
                kept.push(prune_value(item, key, budget));
            }
            Value::Array(kept)
        },
        Value::Object(fields) => {
            let mut kept = serde_json::Map::new();
            for (key, field) in fields {
                if *budget == 0 {
                    break;
                }
                *budget = budget.saturating_sub(key.len());
                kept.insert(key.clone(), prune_value(field, Some(key), budget));
            }
            Value::Object(kept)
        },
        // A number, a bool or a null is already as small as it goes.
        scalar => scalar.clone(),
    }
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
        // nothing. `Interrupt` is Codex's word for a turn the user cancelled, which ends it
        // just as flatly.
        "Stop" | "StopFailure" | "Interrupt" => Some(AgentState::Idle),
        // The tools that are themselves a question. Everything else starting is work; see
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
        // Waiting or nothing, never idle: see [`WAITING_NOTIFICATIONS`] for why no
        // notification is read as the turn ending, `agent_completed` included.
        "Notification" => match event.notification_type.as_deref() {
            Some(kind) if WAITING_NOTIFICATIONS.contains(&kind) => Some(AgentState::Waiting),
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
    let question = || {
        event
            .tool_input
            .as_ref()
            .and_then(|input| input.get("questions")?.get(0))
            .and_then(|question| {
                let text = question.get("question").and_then(|v| v.as_str());
                let header = question.get("header").and_then(|v| v.as_str());
                text.or(header).and_then(clean_line)
            })
            .or_else(|| Some("a question".to_owned()))
    };
    match event.hook_event_name.as_str() {
        "PreToolUse" => match event.tool_name.as_deref() {
            Some("AskUserQuestion" | "request_user_input") => question(),
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

/// Everything an `AskUserQuestion` asks, for a reader with room to show it.
///
/// [`waiting_detail`]'s counterpart for the run history rather than for a toast: the same
/// event, read for what it says instead of for what fits on two lines. Every question, not
/// only the first; each one's header where it adds something the question does not already
/// say; and the options, so a timeline shows what the choice actually was.
///
/// Newlines are kept. The history renders as markdown into a wrapping half-pane, so `- ` in
/// front of an option is a bullet there — and a question flattened into one line, which is
/// what `clean_line` would do, is unreadable as soon as there are two of them.
///
/// Read as defensively as [`waiting_detail`], and for the same reason: `tool_input` has no
/// schema we control. A shape this does not recognise falls back to the one-line detail, so a
/// Claude Code that rearranges the arguments costs the question's *text* rather than the fact
/// that a question was asked.
pub fn question_text(event: &HookEvent) -> Option<String> {
    let questions = event
        .tool_input
        .as_ref()
        .and_then(|input| input.get("questions")?.as_array());
    let mut blocks: Vec<String> = Vec::new();
    for question in questions.into_iter().flatten() {
        let text = question
            .get("question")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let header = question
            .get("header")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|header| !header.is_empty() && Some(*header) != text);
        let mut block = match (header, text) {
            // The header is a chip above the question rather than a sentence, so it goes on
            // its own line rather than being run into one.
            (Some(header), Some(text)) => format!("{header}\n{text}"),
            (None, Some(text)) => text.to_owned(),
            (Some(header), None) => header.to_owned(),
            (None, None) => continue,
        };
        let options = question
            .get("options")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|option| match option {
                // An option is normally `{label, description}`, but a bare string is a shape
                // worth reading rather than dropping.
                Value::String(label) => Some(label.trim()),
                option => option
                    .get("label")
                    .and_then(|value| value.as_str())
                    .map(str::trim),
            })
            .filter(|label| !label.is_empty());
        for label in options {
            block.push_str("\n- ");
            block.push_str(label);
        }
        blocks.push(block);
    }
    match blocks.is_empty() {
        // Nothing recognisable in the arguments — but an `AskUserQuestion` still happened, and
        // the toast's reading of it is the best remaining answer.
        true => waiting_detail(event),
        false => Some(blocks.join("\n\n")),
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
            // Only on a wait, the way the plugin-side feed clears it on every phase that is
            // not one. A record that says `idle` and carries a question is a record that
            // disagrees with itself, and the row, the toast and `agent_list` all read both
            // halves of it.
            waiting_on: (state == AgentState::Waiting)
                .then(|| waiting_detail(event))
                .flatten(),
            // Gated on the state for the same reason: a record that is not a wait has no
            // wait for anybody to answer. `waiting_needs_a_person` is the one place the
            // permission mode is read, so this is where its answer is written down.
            wait_needs_a_person: state == AgentState::Waiting && waiting_needs_a_person(event),
            cwd: event.cwd.clone(),
            session_id: (!event.session_id.is_empty()).then(|| event.session_id.clone()),
            // A hook says everything the agent knows about itself, and the agent does not
            // know whose books it is being kept in. The pane's own run id is carried forward
            // over this — see `AgentInfo::run_id`.
            run_id: None,
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

        let command = "zellij action agent-report --kind codex --hook-json";
        let codex = codex_hook_settings(command);
        let hooks = codex["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), CODEX_HOOK_EVENTS.len());
        for event in CODEX_HOOK_EVENTS {
            assert_eq!(hooks[*event][0]["hooks"][0]["command"], command, "{event}");
            assert_eq!(
                hooks[*event][0]["hooks"][0]["timeout"], CODEX_HOOK_TIMEOUT_SECONDS,
                "{event}"
            );
        }
        assert!(hooks.get("Interrupt").is_some());
        assert!(hooks.get("Notification").is_none());
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
        assert_eq!(hook_state(&event("Interrupt")), Some(AgentState::Idle));
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
        // The one notification that reads like a turn ending and is not: Claude Code raises
        // it for a *background* job this session started, from the same payload the
        // foreground would use. A pane whose agent launched one and worked on would be
        // marked idle mid-turn by it. `Stop` is the turn ending, and says so earlier.
        assert_eq!(hook_state(&notification("agent_completed")), None);

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
                wait_needs_a_person: false,
                cwd: Some("/work".to_owned()),
                session_id: Some("s1".to_owned()),
                run_id: None,
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

        // Codex's question, which is a tool call and not an event of its own.
        let question = HookEvent {
            tool_name: Some("request_user_input".to_owned()),
            tool_input: Some(serde_json::json!({
                "questions": [{"question": "Which history should we keep?"}]
            })),
            ..event("PreToolUse")
        };
        match hook_outcome("codex", &question) {
            HookOutcome::Reported(info) => {
                assert_eq!(info.kind, "codex");
                assert_eq!(info.state, AgentState::Waiting);
                assert_eq!(
                    info.waiting_on.as_deref(),
                    Some("Which history should we keep?")
                );
            },
            other => panic!("{other:?}"),
        }
        // And read the same way whoever asks, because every road that reads an event reads
        // this one function: a kind-keyed answer would reach the record and not the feed.
        assert_eq!(hook_state(&question), Some(AgentState::Waiting));
    }

    /// The record carries [`waiting_needs_a_person`]'s answer, because the pane that draws the
    /// blinking bell is a long way from the event that decided it - and the event is gone by
    /// the time the pane sees anything. Only a wait carries one, for the same reason only a
    /// wait carries `waiting_on`: a record that has moved on has no wait for anybody to
    /// answer.
    #[test]
    fn a_record_says_whether_its_wait_wants_a_person() {
        let request = |tool: &str, mode: &str| HookEvent {
            permission_mode: Some(mode.to_owned()),
            tool_name: Some(tool.to_owned()),
            ..event("PermissionRequest")
        };
        let reported = |event: &HookEvent| match hook_outcome("claude", event) {
            HookOutcome::Reported(info) => info,
            other => panic!("{other:?}"),
        };

        // The mode is about to answer this one itself. The record still says `waiting` - the
        // agent said so, and the row draws what the agent said - but nothing blinks at
        // anybody over it.
        let accepted = reported(&request("Edit", "acceptEdits"));
        assert_eq!(accepted.state, AgentState::Waiting);
        assert!(!accepted.wait_needs_a_person);

        // The same mode, a tool it does not answer.
        let blocked = reported(&request("Bash", "acceptEdits"));
        assert_eq!(blocked.state, AgentState::Waiting);
        assert!(blocked.wait_needs_a_person);

        // A question addressed to a person is one under every mode there is.
        for mode in ["acceptEdits", "bypassPermissions", "dontAsk", "auto"] {
            let asking = HookEvent {
                tool_name: Some("AskUserQuestion".to_owned()),
                permission_mode: Some(mode.to_owned()),
                ..event("PreToolUse")
            };
            assert!(reported(&asking).wait_needs_a_person, "{mode}");
        }

        // And nothing that is not a wait carries one, whatever the mode around it.
        for name in ["PreToolUse", "PostToolUse", "Stop"] {
            let busy = HookEvent {
                permission_mode: Some("default".to_owned()),
                ..event(name)
            };
            let info = reported(&busy);
            assert_ne!(info.state, AgentState::Waiting);
            assert!(!info.wait_needs_a_person, "{name}");
        }
    }

    /// A record that says `idle` and carries a question is a record that disagrees with
    /// itself, and every reader of it — the row, the toast, `agent_list` — reads both halves.
    /// The only source of a wait's text is [`waiting_detail`], which will answer for an
    /// `Elicitation` or a `Notification` whatever the state around it turned out to be.
    #[test]
    fn only_a_wait_carries_what_it_is_waiting_on() {
        let elicitation = HookEvent {
            message: Some("Which account?".to_owned()),
            ..event("Elicitation")
        };
        match hook_outcome("claude", &elicitation) {
            HookOutcome::Reported(info) => {
                assert_eq!(info.state, AgentState::Waiting);
                assert_eq!(info.waiting_on.as_deref(), Some("Which account?"));
            },
            other => panic!("{other:?}"),
        }
        // The same text on an event that is not a wait. `ElicitationResult` is the answer
        // coming back, which is the agent working again.
        let answered = HookEvent {
            message: Some("Which account?".to_owned()),
            ..event("ElicitationResult")
        };
        match hook_outcome("claude", &answered) {
            HookOutcome::Reported(info) => {
                assert_eq!(info.state, AgentState::Busy);
                assert_eq!(info.waiting_on, None);
            },
            other => panic!("{other:?}"),
        }
    }

    /// The broadcast is copied into every running plugin's memory, twice per tool call,
    /// inside a hook with five seconds to live — so a `Write` of a large file must not
    /// travel whole. What a reader takes from `tool_input` is short and must survive.
    #[test]
    fn a_pruned_event_keeps_what_is_read_and_drops_the_rest() {
        let huge = "x".repeat(2 * 1024 * 1024);
        let write = HookEvent {
            tool_name: Some("Write".to_owned()),
            tool_input: Some(serde_json::json!({
                // Deliberately first, the way Claude Code's own schema orders it: cutting
                // every string rather than stopping at a byte count is what keeps the read
                // key alive behind the enormous one.
                "content": huge,
                "file_path": "/work/src/main.rs",
            })),
            last_assistant_message: Some(huge.clone()),
            prompt: Some(huge.clone()),
            ..event("PreToolUse")
        };

        let pruned = write.pruned();
        let json = serde_json::to_string(&pruned).expect("a pruned event serializes");
        assert!(
            json.len() < HOOK_BROADCAST_MAX_BYTES,
            "a pruned event fits the cap, got {}",
            json.len()
        );
        // Every reading still works off the pruned event, which is the whole point of
        // cutting the strings rather than dropping the fields.
        assert_eq!(hook_detail(&pruned).as_deref(), Some("Write main.rs"));
        assert_eq!(hook_state(&pruned), hook_state(&write));
        assert!(pruned
            .last_assistant_message
            .is_some_and(|message| message.chars().count() == PRUNED_MESSAGE_CHARS));

        // The other reader of `tool_input`, whose question is nested two levels down.
        let asking = HookEvent {
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(serde_json::json!({
                "questions": [{ "question": "Which library?", "header": "Library" }],
                "context": huge,
            })),
            ..event("PreToolUse")
        };
        assert_eq!(
            waiting_detail(&asking.pruned()).as_deref(),
            Some("Which library?")
        );

        // An event already inside the cap travels unchanged, which is nearly all of them —
        // `hook_report_actions` does not even call this for one.
        let small = HookEvent {
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(serde_json::json!({ "command": "cargo test" })),
            ..event("PreToolUse")
        };
        assert_eq!(small.pruned(), small);
    }

    /// The prune is a cap in bytes, whatever alphabet the agent was speaking.
    ///
    /// Every string cap here is in characters, and a character is up to four bytes. A plan or
    /// a set of questions written in Japanese, beside three prose fields at their own caps,
    /// is what a single prune could not hold: it went out over the cap the prune exists to
    /// enforce, into every plugin's memory, which is the whole cost the cap is about.
    #[test]
    fn a_broadcast_is_bounded_whatever_alphabet_it_is_in() {
        // Three bytes a character, and every one of them under a key a history reads at
        // length or in a prose field with the largest cap.
        let long = "計".repeat(PRUNED_HISTORY_STRING_CHARS);
        let questions: Vec<serde_json::Value> = (0..8)
            .map(|_| serde_json::json!({ "question": long.clone(), "header": long.clone() }))
            .collect();
        let asking = HookEvent {
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(serde_json::json!({ "questions": questions })),
            last_assistant_message: Some(long.clone()),
            message: Some(long.clone()),
            prompt: Some(long.clone()),
            ..event("PreToolUse")
        };

        let json = serde_json::to_string(&asking).expect("the event serializes");
        assert!(json.len() > HOOK_BROADCAST_MAX_BYTES);
        let payload = asking.broadcast_json(json);
        assert!(
            payload.len() <= HOOK_BROADCAST_MAX_BYTES,
            "the broadcast is bounded, got {}",
            payload.len()
        );
        // Still an event, and still the same event: what a reader takes from it is a name and
        // a first question, both of which fit whatever the budget came down to.
        let arrived: HookEvent =
            serde_json::from_str(&payload).expect("a pruned event is still an event");
        assert_eq!(hook_state(&arrived), hook_state(&asking));
        assert_eq!(arrived.tool_name.as_deref(), Some("AskUserQuestion"));
        assert!(waiting_detail(&arrived).is_some());
    }

    /// An event that already fits travels exactly as it arrived.
    #[test]
    fn a_broadcast_within_the_cap_is_untouched() {
        let event = HookEvent {
            tool_name: Some("Read".to_owned()),
            tool_input: Some(serde_json::json!({ "file_path": "/work/src/main.rs" })),
            ..event("PreToolUse")
        };
        let json = serde_json::to_string(&event).expect("the event serializes");
        assert_eq!(event.broadcast_json(json.clone()), json);
    }

    /// A submitted plan survives a prune, where a passing argument does not.
    ///
    /// The regression the key-aware cut exists for: a plan is `tool_input.plan`, and cutting
    /// every string in `tool_input` to a record's budget turned a long plan into 256
    /// characters on its way to the run history. The prune only runs above
    /// [`HOOK_BROADCAST_MAX_BYTES`], which a big plan and a big anything-else together reach
    /// easily.
    #[test]
    fn a_prune_keeps_a_plan_at_length_and_still_cuts_an_argument() {
        let plan = "step. ".repeat(600);
        let event = HookEvent {
            tool_name: Some("ExitPlanMode".to_owned()),
            tool_input: Some(serde_json::json!({
                "plan": plan,
                "context": "y".repeat(2 * 1024 * 1024),
            })),
            ..event("PreToolUse")
        };
        let pruned = event.pruned();
        let input = pruned.tool_input.expect("the input survives");
        assert_eq!(
            input.get("plan").and_then(|plan| plan.as_str()),
            Some(plan.as_str()),
            "a plan of this length is kept whole"
        );
        assert_eq!(
            input
                .get("context")
                .and_then(|value| value.as_str())
                .map(|value| value.chars().count()),
            Some(PRUNED_STRING_CHARS),
            "an argument nobody reads at length is still cut"
        );
    }

    /// Every question, its header and its options, for the reader that has room for them.
    #[test]
    fn a_question_reaches_the_history_whole_while_the_toast_keeps_its_line() {
        let event = HookEvent {
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(serde_json::json!({
                "questions": [
                    {
                        "header": "Library",
                        "question": "Which library should we use for parsing the manifest?",
                        "options": [
                            { "label": "serde_json", "description": "what everything else uses" },
                            { "label": "simd-json" },
                        ],
                    },
                    { "question": "Keep the old runs?", "options": ["yes", "no"] },
                ],
            })),
            ..event("PreToolUse")
        };

        let text = question_text(&event).expect("a question");
        assert_eq!(
            text,
            "Library\nWhich library should we use for parsing the manifest?\n- serde_json\n- \
             simd-json\n\nKeep the old runs?\n- yes\n- no"
        );

        // The toast and the sidebar are unchanged: one line, and only the first question.
        let line = waiting_detail(&event).expect("a wait");
        assert_eq!(
            line,
            "Which library should we use for parsing the manifest?"
        );
        assert!(line.chars().count() <= WAITING_DETAIL_CHARS);
    }

    /// A shape this does not recognise costs the question's text, not the question.
    #[test]
    fn an_unreadable_question_falls_back_to_the_one_line_detail() {
        let reshaped = HookEvent {
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(serde_json::json!({ "queries": ["which one?"] })),
            ..event("PreToolUse")
        };
        assert_eq!(question_text(&reshaped).as_deref(), Some("a question"));

        let empty = HookEvent {
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(serde_json::json!({ "questions": [{ "options": [] }] })),
            ..event("PreToolUse")
        };
        assert_eq!(question_text(&empty).as_deref(), Some("a question"));
    }

    /// The budget behind the per-string cut, for the shape the cut alone cannot bound: an
    /// object with more keys than a tool call has any business carrying. What it promises is
    /// a size, not any particular key — the keys a reader wants are covered by the cut above.
    #[test]
    fn a_prune_bounds_an_object_no_string_cut_could() {
        let mut input = serde_json::Map::new();
        for i in 0..8192 {
            input.insert(format!("k{i:04}"), serde_json::json!("x".repeat(512)));
        }
        let event = HookEvent {
            tool_name: Some("mcp__something__enormous".to_owned()),
            tool_input: Some(serde_json::Value::Object(input)),
            ..event("PreToolUse")
        };
        let json = serde_json::to_string(&event.pruned()).expect("a pruned event serializes");
        assert!(json.len() < HOOK_BROADCAST_MAX_BYTES, "{}", json.len());
    }

    #[test]
    fn tool_call_identity_accepts_both_provider_spellings_and_survives_pruning() {
        let claude: HookEvent =
            serde_json::from_str(r#"{"hook_event_name":"PreToolUse","tool_use_id":"tool-1"}"#)
                .expect("Claude's spelling parses");
        let alias: HookEvent =
            serde_json::from_str(r#"{"hook_event_name":"PreToolUse","tool_call_id":"tool-2"}"#)
                .expect("the alternate spelling parses");

        assert_eq!(claude.tool_use_id.as_deref(), Some("tool-1"));
        assert_eq!(alias.pruned().tool_use_id.as_deref(), Some("tool-2"));
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

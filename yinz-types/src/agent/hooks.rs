//! What an agent pane says about itself.
//!
//! A pane's agent record (`PaneInfo::agent`) is written by three roads: the spawn path, for a
//! pane a plugin opened as an agent pane; a plugin, through `PluginCommand::SetPaneAgent`; and
//! the agent's own hooks, through `yinz action agent-report --hook-json`. This module reads
//! the common Claude Code/Codex hook shape so the binary and the plugins that draw the result
//! cannot drift apart.

use super::{AgentInfo, AgentSource, AgentState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::path::Path;

/// The Claude Code hook events a record is read from, in the order Claude Code fires them.
///
/// One list for the two things that wire it: `yinz hooks install`, which writes it
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
    "MessageDisplay",
    // Work continues, and names the tool it is in.
    "PreToolUse",
    "PostToolUse",
    // A tool that failed is still the agent working; without this the record sits on the
    // failed tool's name until the next one starts.
    "PostToolUseFailure",
    // Worker lifecycle events feed history without changing the parent's state.
    // Background workers can outlive the parent's turn.
    "SubagentStart",
    "SubagentStop",
    // Compaction is work the user did not ask for and is waiting on all the same.
    "PreCompact",
    "PostCompact",
    // Permission evaluation can be answered by hooks or automatic approval.
    "PermissionRequest",
    // Auto mode refusing a tool call: not a wait, but the agent carrying on, and the
    // record would otherwise still be showing the tool that never ran.
    "PermissionDenied",
    // An MCP server asking for input, and the answer coming back. The `Notification`
    // pair for the same exchange fires about six seconds later; these two are the
    // instant version of it.
    "Elicitation",
    "ElicitationResult",
    // Explicit input, elicitation, and permission prompt notifications.
    "Notification",
    // Stop hooks run before the turn ends; transcript confirmation restores idle. It carries
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
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
    "Interrupt",
    "Stop",
    "SessionEnd",
];

/// The Muse hook events used to report a pane's state.
///
/// `Notification` fires when the agent needs approval, so it reads as the wait it is:
/// the host translates it to `Elicitation` on the way in, because the shared
/// [`hook_state`] cannot tell one agent's notification from another's. `PreLLMCall`
/// and `PostLLMCall` are deliberately not wired: they carry the whole conversation
/// on every model call.
pub const MUSE_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
    "Notification",
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

/// Seconds a Muse hook may take. Muse hook timeouts are seconds, not milliseconds,
/// and a report is one local `yinz` invocation - five is the same bound Claude Code's
/// hooks run under, for the same reason (a hook blocks the turn).
pub const MUSE_HOOK_TIMEOUT_SECONDS: u64 = 5;

/// The command a spawned Claude Code pane's hooks run, with no script in between: the pane
/// reports through the broker when remote, otherwise through the local session.
/// Without yinz on PATH, installed hooks can still report through their absolute binary path.
pub const CLAUDE_HOOK_REPORT_COMMAND: &str = r#"command -v yinz >/dev/null 2>&1 || exit 0; if [ -n "$YINZ_REMOTE_PANE_TOKEN" ]; then yinz --pty-broker report claude; else yinz action agent-report --hook-json; fi"#;

/// The pipe the raw event is broadcast on beside the record, and the `--args` keys naming the
/// pane it fired in and the agent it came from.
///
/// `agent-report --hook-json` sets the pane's record *and* hands the event itself to whatever
/// plugin wants more than a record holds - agent's daemon reads `last_assistant_message` off a
/// `SessionEnd` to know a run is over and worth summarising, and tool arguments to keep
/// its history. Spelled here rather than in the plugin crate so
/// the two ends cannot drift: a pipe name that does not match is silent.
///
/// The kind travels because the payload cannot carry it: a hook event is the agent's own JSON
/// and says nothing about which agent wrote it, and the daemon writes a record of its own from
/// what arrives here. Absent - an old script that only pipes - it is a `claude`, which is what
/// every such script was installed for.
/// One event from an agent's own hooks, as the session hands it to plugins: `Event::AgentEvent`,
/// which `yinz action agent-report --hook-json` sends beside the record it sets, and which
/// `yinz events --type agent_event` streams.
///
/// The fields a reader wants are derived here, once - `state` by [`hook_state_for`], `detail` by
/// [`hook_detail_for`] - so a plugin never re-reads Claude Code's schema; `raw` is the agent's own
/// JSON, bounded by [`HOOK_BROADCAST_MAX_BYTES`], for the one reader that keeps a run's history
/// and needs more than these say (`tool_input`, the permission mode, the subagent). The two
/// prose fields, `prompt` and `last_message`, are read off the same bounded event `raw` was
/// written from: an event that fit keeps them whole, and one that was pruned has them cut the
/// way `raw` has - the cap is on the whole event, not on one field of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvent {
    /// The terminal pane the agent runs in.
    pub pane_id: crate::internal_ids::TerminalId,
    /// `claude`, `codex`, … as the report named it.
    pub kind: String,
    /// The agent's own name for the event: `SessionStart`, `UserPromptSubmit`, `Stop`, …
    pub hook: String,
    /// The agent's own session id, when the event carries one.
    pub session_id: Option<String>,
    /// What the event says the agent is now doing, when it says anything.
    pub state: Option<AgentState>,
    /// The tool it names, cut to a few words - what a `busy · Bash cargo` row shows.
    pub detail: Option<String>,
    /// What the user asked for, on a `UserPromptSubmit`. Cut exactly as `raw`'s copy is.
    pub prompt: Option<String>,
    /// What the agent said last, on a `Stop` or `StopFailure`. Cut exactly as `raw`'s copy is.
    pub last_message: Option<String>,
    /// The tool about to run or just finished.
    pub tool_name: Option<String>,
    /// The folder the agent is working in.
    pub cwd: Option<String>,
    /// The event as the agent wrote it, pruned over the cap. See [`HookEvent`].
    pub raw: String,
}

impl AgentEvent {
    /// Read `event` the way the record is read - one reading, so a plugin and the pane's
    /// record cannot disagree about what the event meant - and keep `json` as it arrived.
    pub fn from_hook(
        kind: &str,
        pane_id: crate::internal_ids::TerminalId,
        event: &HookEvent,
        json: String,
    ) -> Self {
        // The prose comes off the bounded event, not `event`: a prompt or a final message has
        // no bound in Claude Code's schema, and copying it whole beside a pruned `raw` would
        // hand every subscribing plugin the very bytes the prune exists to keep out.
        let (bounded, raw) = event.broadcast(json);
        AgentEvent {
            pane_id,
            kind: kind.to_owned(),
            hook: event.hook_event_name.clone(),
            session_id: Some(event.session_id.clone()).filter(|id| !id.is_empty()),
            state: hook_state_for(kind, event),
            detail: hook_detail_for(kind, event),
            prompt: bounded.prompt.clone(),
            last_message: bounded.last_assistant_message.clone(),
            tool_name: event.tool_name.clone(),
            cwd: event.cwd.clone(),
            raw,
        }
    }
}

/// One command handler entry, as Claude Code and Codex spell it.
pub fn hook_handler(command: &str, timeout: u64) -> Value {
    json!({
        "type": "command",
        "command": command,
        "timeout": timeout,
    })
}

/// A Claude Code settings document whose `hooks` wire every event in [`CLAUDE_HOOK_EVENTS`]
/// to `command`. What `yinz hooks install` merges into a settings file, and what a
/// spawned pane is handed whole as `--settings`; the two cannot disagree about the events
/// because this is the one place they are listed.
pub fn claude_hook_settings(command: &str) -> Value {
    hook_settings(CLAUDE_HOOK_EVENTS, command, CLAUDE_HOOK_TIMEOUT_SECONDS)
}

/// A Codex `hooks.json` document wiring every event in [`CODEX_HOOK_EVENTS`].
pub fn codex_hook_settings(command: &str) -> Value {
    hook_settings(CODEX_HOOK_EVENTS, command, CODEX_HOOK_TIMEOUT_SECONDS)
}

/// A Muse `settings.json` hooks block wiring every event in [`MUSE_HOOK_EVENTS`].
/// What `yinz hooks install` merges into the user's settings file.
pub fn muse_hook_settings(command: &str) -> Value {
    hook_settings(MUSE_HOOK_EVENTS, command, MUSE_HOOK_TIMEOUT_SECONDS)
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

/// The internal flag behind which `yinz` runs a command in a prepared Bubblewrap
/// environment. Launching and recognition share it so neither can drift from the other.
pub const BUBBLEWRAP_RUN_FLAG: &str = "--bubblewrap-run";

/// The command a Bubblewrap pane runs, read from the argv of the process holding its pty:
/// this binary's launcher until it execs, then `bwrap`. Both end in `-- <command>`.
///
/// For a pane already known to be sandboxed. Launch metadata decides that
/// (`PaneTarget::Bubblewrap`), never the shape of a command.
pub fn bubblewrap_inner_argv(argv: &[String]) -> Option<&[String]> {
    let inner = match argv {
        [_, flag, _root, separator, inner @ ..]
            if flag == BUBBLEWRAP_RUN_FLAG && separator == "--" =>
        {
            inner
        },
        [bwrap, options @ ..] if program(bwrap) == "bwrap" => {
            let separator = options.iter().position(|option| option == "--")?;
            &options[separator + 1..]
        },
        _ => return None,
    };
    (!inner.is_empty()).then_some(inner)
}

/// The program and arguments behind an `env(1)` or `yinz agent` wrapper.
/// Other commands are returned unchanged.
pub fn unwrapped_program_and_args<'a>(
    program: &'a Path,
    args: &'a [String],
) -> Option<(&'a Path, &'a [String])> {
    if program.file_name()? == "yinz" {
        if let [agent, launcher, flag, command, separator, rest @ ..] = args
            && agent == "agent"
            && matches!(
                launcher.as_str(),
                "codex" | "codex-untracked" | "claude" | "muse"
            )
            && flag == "--command"
            && separator == "--"
        {
            return Some((Path::new(command), rest));
        }
        if let [agent, launcher, separator, rest @ ..] = args
            && agent == "agent"
            && matches!(
                launcher.as_str(),
                "codex" | "codex-untracked" | "claude" | "muse"
            )
            && separator == "--"
        {
            let program = if launcher == "codex-untracked" {
                "codex"
            } else {
                launcher
            };
            return Some((Path::new(program), rest));
        }
    }
    if program.file_name()? != "env" {
        return Some((program, args));
    }
    // `env`'s own options and assignments come first; the program is the first word that
    // is neither. `-u NAME` takes a value, which is skipped with it.
    let mut skip_next = false;
    for (index, arg) in args.iter().enumerate() {
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
        return unwrapped_program_and_args(Path::new(arg), &args[index + 1..]);
    }
    None
}

/// The program a command pane's agent record is named for: the command's file name, looked
/// through an `env(1)` wrapper - `env K=V claude …` is `claude`, because `env` execs it and the
/// pane is running the agent, not the wrapper. `None` when there is no program to name.
pub fn agent_kind_of(program: &Path, args: &[String]) -> Option<String> {
    if let Some(kind) = kind_from_program_and_args(program, args)
        && super::provider::provider(&kind).is_some()
    {
        return Some(kind);
    }
    unwrapped_program_and_args(program, args)
        .and_then(|(program, _)| program.file_name())
        .map(|name| name.to_string_lossy().to_string())
}

/// Every agent we know.
///
/// Canonical names only. The `-cli`, `-code` and `-agent` aliases this used to spell
/// out are what `kind_of` gets from the leading dash-segment, along with the ones no
/// table could have listed: an installer that drops `muse-bin-0.1.0-R708.1` on the
/// path names the same agent as one that drops `muse`.
///
/// One list for everyone who asks what an agent is: `agent-service`, which latches a pane's
/// "detected" kind from its command, and the server, which decides where a spawned agent
/// pane goes before the process has said anything. Two lists would be two answers.
pub const KINDS: &[&str] = &[
    "muse", "codex", "claude", "cursor", "qwen", "gemini", "aider", "cline", "pi", "agy",
];

/// Programs that run another program named in their own arguments. Half the table
/// above ships as a `#!/usr/bin/env node` shim, so the real argv is
/// `["node", "/opt/bin/gemini"]` and the first word names no agent at all.
pub(super) const INTERPRETERS: &[&str] = &[
    "node", "bun", "deno", "npx", "python", "python3", "ruby", "perl", "uv", "uvx",
];

/// Programs whose foreground command means that the pane is at a shell prompt.
pub const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "csh", "nu", "xonsh", "elvish", "pwsh",
    "ash", "busybox",
];

/// The program a command-line word names: the file name of its path.
pub fn program(word: &str) -> &str {
    Path::new(word)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(word)
}

/// The kind a program name is: its file name, then that without an extension, then
/// its leading dash-segment.
///
/// Each step strips decoration a packager added and the user never typed — `claude.js`
/// from a shim, `-0.1.0-R708.1` from a versioned binary, `-cli` from a published name.
/// The segment is still a whole one, so `pipenv` is not `pi`.
pub fn kind_of(word: &str) -> Option<String> {
    let name = program(word).to_lowercase();
    let stem = name.split('.').next().unwrap_or_default();
    [
        name.as_str(),
        stem,
        stem.split('-').next().unwrap_or_default(),
    ]
    .iter()
    .find_map(|candidate| {
        if *candidate == "antigravity" {
            Some("agy".to_owned())
        } else {
            KINDS
                .iter()
                .find(|kind| *kind == candidate)
                .map(|kind| (*kind).to_owned())
                .or_else(|| {
                    super::provider::provider(candidate).map(|provider| provider.kind.clone())
                })
        }
    })
}

/// The agent kind a command line names, or `None`.
///
/// Whole tokens, never a substring: `pi` against a substring would make every `pip
/// install` shell an agent, and an agent is something the model may send input to.
///
/// This is the half of detection that can be *trusted over time*: a pane that is
/// running an agent right now is one, whatever it is called. `agent-service` latches
/// on this and nothing else.
pub fn kind_from_command(command: &str) -> Option<String> {
    let mut words = command.split_whitespace();
    let first = words.next().unwrap_or_default();
    if program(first) == "yinz" && words.next() == Some("agent") {
        // The launchers only: `yinz hooks install` is maintenance, not an agent.
        match words.next() {
            Some("codex" | "codex-untracked") => return Some("codex".to_owned()),
            Some("claude") => return Some("claude".to_owned()),
            Some("muse") => return Some("muse".to_owned()),
            _ => {},
        }
    }
    if let Some(kind) = kind_of(first) {
        return Some(kind);
    }
    // Later words count only behind an interpreter. Otherwise `git commit -m
    // "ask claude"` would be an agent, which is the same reason detection is
    // token-based rather than substring-based.
    if INTERPRETERS.contains(&program(first).to_lowercase().as_str())
        && let Some(kind) = words.find_map(kind_of)
    {
        return Some(kind);
    }
    None
}

/// The agent kind a command the session holds as a program and its arguments names, or
/// `None` - a `RunCommand` about to become a pane.
///
/// Reads the *program* and nothing else: the `env(1)`-unwrapped program word, or, behind an
/// interpreter, the script the interpreter runs. [`kind_from_command`] scans every word behind
/// an interpreter, and that is fine for the label `agent-service` puts on a pane, but this
/// decides where the pane *goes*, and a `python3 gemini_test.py --api gemini` moved to another
/// tab costs the user a layout, where a wrong label costs nothing.
///
/// `env` is looked through the way [`agent_kind_of`] does, because that is how a plugin hands
/// an agent its environment, and `env` is not an interpreter in the sense above: its program
/// is not among its own arguments' interpreters but after its assignments.
pub fn kind_from_program_and_args(program_path: &Path, args: &[String]) -> Option<String> {
    let (program_path, args) = unwrapped_program_and_args(program_path, args)?;
    let name = program_path.file_name()?.to_string_lossy();
    if let Some(kind) = kind_of(&name) {
        return Some(kind);
    }
    if INTERPRETERS.contains(&name.to_lowercase().as_str()) {
        return interpreted_script(&name.to_lowercase(), args).and_then(kind_of);
    }
    None
}

/// The script following supported interpreter options and an optional runner `run` subcommand.
///
/// `args` starts after the interpreter; any `env` wrapper has already been removed.
fn interpreted_script<'a>(interpreter: &str, args: &'a [String]) -> Option<&'a str> {
    let mut words = args.iter().map(String::as_str).peekable();
    if matches!(interpreter, "uv" | "bun" | "deno") && words.peek() == Some(&"run") {
        words.next();
    }
    for arg in words {
        let supported_option = match interpreter {
            "python" | "python3" => arg == "-u",
            "node" => arg.starts_with("--max-old-space-size="),
            "deno" => arg == "--allow-all",
            _ => false,
        };
        if supported_option {
            continue;
        }
        // ponytail: unknown options may consume values or execute inline code; add explicit
        // support with regression tests when another launch form needs automatic placement.
        return (!arg.starts_with('-')).then_some(arg);
    }
    None
}

/// The event name that ends a session, and so ends the pane's record.
pub const HOOK_SESSION_END: &str = "SessionEnd";

/// The `Notification` kinds that mean the agent is blocked on the user.
/// Claude permission prompts are handled separately in [`hook_state_for`].
/// Worker permission reminders lack the identity needed to clear a worker wait.
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
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
];

/// The tools whose `PreToolUse` *is* the question going up.
///
/// Every other tool starting is work, but these three do nothing except put a prompt in front
/// of the user and wait. Their `PostToolUse` is ordinary work again, which is what clears the
/// wait - so a tool named here needs no special case on the way out.
///
/// `request_user_input` is Codex's equivalent. Every reporting path reads this list through
/// [`hook_state_for`], including the pane record and Agent's feed.
pub const WAITING_TOOLS: &[&str] = &[
    "AskUserQuestion",
    "ExitPlanMode",
    "request_user_input",
    "ask_question",
];

/// Permission modes whose automatic approval is work, not a wait for user input.
/// `acceptEdits` is handled separately because it only answers file-editing prompts.
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

/// The maximum serialized hook JSON carried in `raw`, delivered only to subscribers.
/// The surrounding event also carries typed fields and can be larger.
pub const HOOK_BROADCAST_MAX_BYTES: usize = 256 * 1024;

/// The largest native provider hook payload an intake accepts before parsing.
///
/// The broadcast cap bounds the *normalized* event, not the provider's own bytes: a
/// prompt `agent prompt` accepted (up to 1 MiB) arrives whole inside
/// `UserPromptSubmit`, so an intake that applied the broadcast cap to the native
/// payload would discard the acknowledgement before the prune could preserve its
/// fingerprint. Oversized-but-valid payloads are parsed and pruned to broadcast
/// size; anything past this ceiling is not a hook event.
pub const NATIVE_HOOK_MAX_BYTES: usize = 2 * 1024 * 1024;

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
const PRUNED_HISTORY_STRING_CHARS: usize = HOOK_BROADCAST_MAX_BYTES;

/// Maximum per-field byte budget searched when the serialized hook exceeds the ceiling.
const PRUNED_INPUT_BUDGET: usize = HOOK_BROADCAST_MAX_BYTES;

/// Prose can use the full budget; serialization and other fields decide the actual cut.
const PRUNED_MESSAGE_CHARS: usize = HOOK_BROADCAST_MAX_BYTES;

/// Background task IDs one Claude turn can report; IDs past this are not tracked.
pub const MAX_BACKGROUND_TASKS: usize = 32;

/// A Claude Code hook event, as its JSON arrives.
///
/// Every field but the event name is optional, because the payload's shape differs between
/// events and between versions, and an unrecognised one must not cost us the event.
///
/// Deliberately *not* named for `input::hooks::HookPoint`, which is a different mechanism
/// entirely — that one is yinz handing a moment of its own to a plugin.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEvent {
    /// `SessionStart`, `PreToolUse`, `Stop`, … — see [`hook_state`].
    pub hook_event_name: String,
    /// Claude Code's own session id. Kept so a pane that is restarted in place can be told
    /// from one that merely went quiet.
    #[serde(default)]
    pub session_id: String,
    /// Provider turn identity, preserved even when prose is pruned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Claude's submitted prompt identity and host-side turn confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    #[serde(
        default,
        rename = "_yinz_claude_turn_complete",
        skip_serializing_if = "Option::is_none"
    )]
    pub claude_turn_complete: Option<bool>,
    /// Background tasks (Bash `run_in_background` or async subagents) that the confirmed Claude
    /// turn launched. Each finishes later in its own `<task-notification>` turn.
    #[serde(
        default,
        rename = "_yinz_claude_background_tasks",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub claude_background_tasks: Vec<String>,
    /// Antigravity's host-side turn confirmation.
    #[serde(
        default,
        rename = "_yinz_agy_turn_complete",
        skip_serializing_if = "Option::is_none"
    )]
    pub agy_turn_complete: Option<bool>,
    /// [`crate::agent::control::prompt_sha256`] of the prompt, taken before pruning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_sha256: Option<String>,
    /// Bridge selection announcement; only verified idle Codex conversations may bootstrap readiness.
    #[serde(
        default,
        rename = "_yinz_codex_hooks_ready",
        skip_serializing_if = "Option::is_none"
    )]
    pub codex_hooks_ready: Option<bool>,
    /// Launcher verification of installed Muse hooks, not composer readiness.
    /// Unpinned: Muse speaks no session id until its first turn begins,
    /// so the bootstrap carries an empty one and the first submit binds it.
    #[serde(
        default,
        rename = "_yinz_muse_hooks_ready",
        skip_serializing_if = "Option::is_none"
    )]
    pub muse_hooks_ready: Option<bool>,
    /// Why a `SessionStart` fired: `startup`, `resume`, `clear`, `compact`, or `fork`.
    #[serde(default)]
    pub source: Option<String>,
    /// The folder Claude Code is working in, which is not always the pane's shell folder —
    /// the agent is what moved, and this is the one that says where to.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Which notification this is, on a `Notification`. Most of them say nothing about whether
    /// the user is being waited on; [`hook_state_for`] is where the ones that do are named.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_response: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_interrupt: Option<bool>,
    #[serde(default, alias = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_capture: Option<super::tool_capture::ToolCapture>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archive_only: bool,
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
    /// Providers other than Claude use it to distinguish automatic permission approval.
    /// Claude permission evaluation stays busy; its permission notification establishes a wait.
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Codex's permission reviewer, supplied by the hook or its matching transcript turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<String>,
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
    /// The subagent's type on a Claude subagent hook, such as `general-purpose`.
    ///
    /// Claude reports an empty type for its own hidden helpers, such as the prompt-suggestion
    /// subagent, which no caller launched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    #[serde(default, rename = "final", skip_serializing_if = "Option::is_none")]
    pub final_batch: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub message_incomplete: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assistant_messages: Vec<CapturedMessage>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub messages_omitted: bool,
}

/// Completed assistant prose extracted on the agent's host.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedMessage {
    pub source_id: String,
    pub turn_id: String,
    pub text: String,
    #[serde(default)]
    pub incomplete: bool,
}

impl HookEvent {
    pub fn is_worker(&self) -> bool {
        self.agent_id.is_some()
            || matches!(
                self.hook_event_name.as_str(),
                "SubagentStart" | "SubagentStop"
            )
    }

    /// This event, cut to a size worth broadcasting. See [`HOOK_BROADCAST_MAX_BYTES`].
    ///
    /// Every field survives — what is cut is how much of each one does. Strings first and a
    /// budget second, deliberately in that order: the keys a reader actually looks at inside
    /// `tool_input` are `command`, `file_path` and `questions[0]`, none of which is long, so
    /// cutting each string to [`PRUNED_STRING_CHARS`] leaves them all intact whatever order
    /// they arrived in. The budget only ever bites on an object with more keys than a tool
    /// call has any business carrying, and it drops the tail rather than the head.
    ///
    /// Unknown provider fields and raw responses are dropped. The archive-specific
    /// `tool_capture` was prepared before pruning and survives with its own bounds.
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
        let mut remaining = budget;
        let mut retained_messages = Vec::new();
        for message in self.assistant_messages.iter().rev() {
            let mut message = message.clone();
            let text = truncate_within(&message.text, PRUNED_MESSAGE_CHARS, remaining / 2);
            message.incomplete |= text != message.text;
            message.text = text;
            let size = serde_json::to_vec(&message).map_or(usize::MAX, |json| json.len());
            if size > remaining {
                break;
            }
            remaining -= size;
            retained_messages.push(message);
        }
        retained_messages.reverse();
        HookEvent {
            hook_event_name: self.hook_event_name.clone(),
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone(),
            prompt_id: self.prompt_id.clone(),
            transcript_path: self.transcript_path.clone(),
            claude_turn_complete: self.claude_turn_complete,
            claude_background_tasks: self
                .claude_background_tasks
                .iter()
                .take(MAX_BACKGROUND_TASKS)
                .map(|task| truncate_within(task, 128, 128))
                .collect(),
            agy_turn_complete: self.agy_turn_complete,
            prompt_sha256: self.prompt_sha256.clone(),
            codex_hooks_ready: self.codex_hooks_ready,
            muse_hooks_ready: self.muse_hooks_ready,
            source: self.source.clone(),
            cwd: self.cwd.clone(),
            notification_type: self.notification_type.clone(),
            tool_use_id: self.tool_use_id.clone(),
            tool_name: self.tool_name.clone(),
            tool_response: None,
            error: None,
            is_interrupt: self.is_interrupt,
            is_error: self.is_error,
            duration_ms: self.duration_ms,
            parent_tool_use_id: self.parent_tool_use_id.clone(),
            tool_capture: self.tool_capture.clone(),
            archive_only: self.archive_only,
            tool_input: self.tool_input.as_ref().map(|input| {
                let mut budget = budget;
                prune_value(input, None, &mut budget)
            }),
            last_assistant_message: cut(&self.last_assistant_message, PRUNED_MESSAGE_CHARS),
            message: cut(&self.message, PRUNED_MESSAGE_CHARS),
            permission_mode: self.permission_mode.clone(),
            approvals_reviewer: self.approvals_reviewer.clone(),
            prompt: cut(&self.prompt, PRUNED_MESSAGE_CHARS),
            agent_id: self.agent_id.clone(),
            agent_type: self.agent_type.clone(),
            message_id: self.message_id.clone(),
            index: self.index,
            final_batch: self.final_batch,
            delta: cut(&self.delta, PRUNED_MESSAGE_CHARS),
            message_incomplete: self.message_incomplete
                || cut(&self.delta, PRUNED_MESSAGE_CHARS) != self.delta,
            messages_omitted: self.messages_omitted
                || retained_messages.len() < self.assistant_messages.len(),
            assistant_messages: retained_messages,
        }
    }

    /// This event as the bytes to broadcast, at or under [`HOOK_BROADCAST_MAX_BYTES`].
    ///
    /// `json` is the event exactly as it arrived, which is what goes out whenever it fits —
    /// nearly always. Over the cap it is re-serialized from a prune, and the prune is repeated
    /// with a bounded binary search for the largest budget that fits. An input cap cannot
    /// promise an output size, because JSON escaping expands what it encodes — six bytes for one
    /// control character — and there are several fields drawing on the budget. The last step
    /// spends nothing on prose. Pathological metadata is bounded separately. A pruned prompt
    /// is carried with its pasted content unwrapped; its `prompt_sha256` is of the whole one.
    pub fn broadcast_json(&self, json: String) -> String {
        self.broadcast(json).1
    }

    /// [`broadcast_json`](Self::broadcast_json), with the event the bytes were written from
    /// beside them: `self` when `json` fit, the prune otherwise. A field read off it says what
    /// `raw` says, which is how [`AgentEvent::from_hook`] keeps its typed prose under the
    /// same cap as the JSON it travels with.
    pub fn broadcast(&self, json: String) -> (Cow<'_, HookEvent>, String) {
        if json.len() <= HOOK_BROADCAST_MAX_BYTES {
            return (Cow::Borrowed(self), json);
        }
        let mut source = self.clone();
        if let Some(prompt) = self
            .prompt
            .as_ref()
            .filter(|_| self.prompt_sha256.is_none())
        {
            source.prompt_sha256 = Some(crate::agent::control::prompt_sha256(prompt));
        }
        // Unwrapped before it is cut: a cut can drop a collapsed paste's closer, and an
        // opener with no closer stays in the prompt readers show.
        if let Some(prompt) = &self.prompt {
            source.prompt = Some(crate::agent::control::unwrap_pasted_content(prompt).into_owned());
        }
        let mut low = 0;
        let mut high = PRUNED_INPUT_BUDGET;
        let mut best = source.pruned_to(0);
        let mut raw = serde_json::to_string(&best).expect("hook fields serialize");
        // Metadata is normally tiny. Bound pathological metadata too so even the
        // zero-prose candidate respects the JSON ceiling.
        if raw.len() > HOOK_BROADCAST_MAX_BYTES {
            let mut value = serde_json::to_value(&best).expect("hook fields serialize");
            for field in value
                .as_object_mut()
                .expect("hook is an object")
                .values_mut()
            {
                if let Value::String(text) = field {
                    *text = truncate_within(text, 1024, 1024);
                }
            }
            best = serde_json::from_value(value).expect("same hook fields");
            raw = serde_json::to_string(&best).expect("hook fields serialize");
            return (Cow::Owned(best), raw);
        }
        while low < high {
            let budget = low + (high - low).div_ceil(2);
            let pruned = source.pruned_to(budget);
            let candidate = serde_json::to_string(&pruned).expect("hook fields serialize");
            if candidate.len() <= HOOK_BROADCAST_MAX_BYTES {
                low = budget;
                best = pruned;
                raw = candidate;
            } else {
                high = budget - 1;
            }
        }
        (Cow::Owned(best), raw)
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
/// `run_entry` in agent-proto — and the history has a wrapping, scrolling half-pane to show
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
/// Claude and Antigravity Stop hooks run before blocking hooks can continue the turn.
pub fn hook_state_for(kind: &str, event: &HookEvent) -> Option<AgentState> {
    if event.archive_only {
        return None;
    }
    if kind == "pi"
        && matches!(
            event.hook_event_name.as_str(),
            "ElicitationResult" | "PostCompact"
        )
        && event.source.as_deref() == Some("idle")
    {
        return Some(AgentState::Idle);
    }
    if event.hook_event_name == "Stop"
        && ((kind == "claude" && event.claude_turn_complete != Some(true))
            || (kind == "agy" && event.agy_turn_complete != Some(true)))
    {
        return Some(AgentState::Busy);
    }
    if kind == "claude" {
        match event.hook_event_name.as_str() {
            // Claude uses trigger=auto both during a turn and while idle. Neither
            // compaction hook establishes a turn boundary, so preserve the last
            // reported state (also when no managed-agent lifecycle exists).
            // SessionStart(source=compact) is already stateless for the same reason.
            "PreCompact" | "PostCompact" => return None,
            "Stop" if event.claude_turn_complete != Some(true) => {
                return Some(AgentState::Busy);
            },
            // Hooks and automatic approval can answer a permission request before a
            // person sees it. Dedicated input tools already established their wait.
            "PermissionRequest" => {
                return (!event
                    .tool_name
                    .as_deref()
                    .is_some_and(|tool| WAITING_TOOLS.contains(&tool)))
                .then_some(AgentState::Busy);
            },
            "Notification" if event.notification_type.as_deref() == Some("permission_prompt") => {
                return Some(AgentState::Waiting);
            },
            _ => {},
        }
    }
    hook_state(event)
}

pub fn hook_state(event: &HookEvent) -> Option<AgentState> {
    if event.archive_only {
        return None;
    }
    match event.hook_event_name.as_str() {
        "SessionStart"
            if matches!(
                event.source.as_deref(),
                Some("startup" | "resume" | "fork" | "clear")
            ) =>
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
        // An MCP server asking for input, said as the form goes up.
        "Elicitation" => Some(AgentState::Waiting),
        "PermissionRequest" => Some(if waiting_needs_a_person(event) {
            AgentState::Waiting
        } else {
            AgentState::Busy
        }),
        // Workers can start or finish after the parent stops, or while it waits for input.
        "SubagentStart" | "SubagentStop" => None,
        // Work by the parent, including compaction.
        "UserPromptSubmit" | "PreInvocation" | "PostInvocation" | "PreToolUse" | "PostToolUse"
        | "PostToolUseFailure" | "PermissionDenied" | "ElicitationResult" | "PreCompact"
        | "PostCompact" => Some(AgentState::Busy),
        // Waiting or nothing, never idle: see [`WAITING_NOTIFICATIONS`] for why no
        // notification is read as the turn ending, `agent_completed` included.
        "Notification" => match event.notification_type.as_deref() {
            Some(kind) if WAITING_NOTIFICATIONS.contains(&kind) => Some(AgentState::Waiting),
            _ => None,
        },
        _ => None,
    }
}

/// Whether this event needs a person. Automatic permission approval is busy;
/// explicit questions and elicitation always wait, regardless of approval mode.
/// Missing or unknown modes, and unknown tools under `acceptEdits`, stay manual.
pub fn waiting_needs_a_person(event: &HookEvent) -> bool {
    if event.hook_event_name != "PermissionRequest" {
        return true;
    }
    if event.approvals_reviewer.as_deref() == Some("auto_review") {
        return false;
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

/// The tool hint for `PreToolUse` or permission evaluation classified as busy.
///
/// The tool's own name, and one argument's worth of hint where the tool has an obvious one:
/// which file, or which program. Everything else about `tool_input` is ignored rather than
/// summarised — it is free-form text of arbitrary size that the agent's *input* can steer, so
/// what little is taken is cut to [`HOOK_DETAIL_CHARS`] and stripped of anything that is not
/// printable.
///
/// `None` for other events, which clears the hint when the tool finishes.
pub fn hook_detail_for(kind: &str, event: &HookEvent) -> Option<String> {
    if event.hook_event_name == "PermissionRequest"
        && hook_state_for(kind, event) == Some(AgentState::Busy)
    {
        tool_summary(event)
    } else {
        hook_detail(event)
    }
}

pub fn hook_detail(event: &HookEvent) -> Option<String> {
    if event.hook_event_name != "PreToolUse"
        && (event.hook_event_name != "PermissionRequest" || waiting_needs_a_person(event))
    {
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
        "run_command" => input
            .and_then(|input| input.get("CommandLine")?.as_str())
            .and_then(|command| command.split_whitespace().next()),
        "Read" | "Edit" | "Write" | "NotebookEdit" => input
            .and_then(|input| input.get("file_path")?.as_str())
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(path)
            }),
        "view_file" => input
            .and_then(|input| input.get("AbsolutePath")?.as_str())
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(path)
            }),
        "write_to_file" | "replace_file_content" => input
            .and_then(|input| input.get("TargetFile")?.as_str())
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
            Some("AskUserQuestion" | "request_user_input" | "ask_question") => question(),
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
    if event.is_worker() || event.archive_only {
        return HookOutcome::Nothing;
    }
    if event.hook_event_name == HOOK_SESSION_END {
        return HookOutcome::Ended;
    }
    match hook_state_for(kind, event) {
        Some(state) => HookOutcome::Reported(AgentInfo {
            kind: kind.to_owned(),
            state,
            source: AgentSource::Reported,
            detail: hook_detail_for(kind, event),
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
#[path = "unit/hooks_tests.rs"]
mod tests;

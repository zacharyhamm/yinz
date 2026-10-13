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
use std::path::{Path, PathBuf};

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

/// Generated mother banner wrapper, shared with detection so lifecycle hooks still see the agent.
pub const MOTHER_FALLBACK_SCRIPT: &str = "printf '%s\\n' \"$1\"; shift; exec \"$@\"";

/// Wrap a local mother command with its one-time fallback warning.
pub fn mother_fallback(program: &mut PathBuf, args: &mut Vec<String>, reason: &str) {
    let message = format!(
        "Mother is running unsandboxed (no filesystem isolation): {}",
        crate::text::clean_string_from_control_and_linebreak(reason)
    );
    let mut wrapped = vec![
        "-c".into(),
        MOTHER_FALLBACK_SCRIPT.into(),
        "mother".into(),
        message,
        program.to_string_lossy().into_owned(),
    ];
    wrapped.append(args);
    *program = "/bin/sh".into();
    *args = wrapped;
}

/// Index of the executable in our generated banner wrapper, never arbitrary shell code.
pub fn mother_fallback_command_index(program: &Path, args: &[String]) -> Option<usize> {
    (program.file_name().is_some_and(|name| name == "sh")
        && matches!(args, [flag, script, name, _reason, _command, ..]
            if flag == "-c" && script == MOTHER_FALLBACK_SCRIPT && name == "mother"))
    .then_some(4)
}

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

/// The program and arguments behind an `env(1)`, `yinz agent`, or mother banner wrapper.
/// Other commands are returned unchanged.
pub fn unwrapped_program_and_args<'a>(
    program: &'a Path,
    args: &'a [String],
) -> Option<(&'a Path, &'a [String])> {
    if let Some(index) = mother_fallback_command_index(program, args) {
        return unwrapped_program_and_args(Path::new(&args[index]), &args[index + 1..]);
    }
    if program.file_name()? == "yinz" {
        if let [agent, launcher, flag, command, separator, rest @ ..] = args {
            if agent == "agent"
                && matches!(
                    launcher.as_str(),
                    "codex" | "codex-untracked" | "claude" | "muse"
                )
                && flag == "--command"
                && separator == "--"
            {
                return Some((Path::new(command), rest));
            }
        }
        if let [agent, launcher, separator, rest @ ..] = args {
            if agent == "agent"
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
    if let Some(kind) = kind_from_program_and_args(program, args) {
        if super::provider::provider(&kind).is_some() {
            return Some(kind);
        }
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
    if INTERPRETERS.contains(&program(first).to_lowercase().as_str()) {
        if let Some(kind) = words.find_map(kind_of) {
            return Some(kind);
        }
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
mod tests {
    use super::*;

    #[test]
    fn worker_hooks_only_broadcast_without_a_parent_state_change() {
        for kind in ["claude", "codex", "muse"] {
            for name in [
                "SubagentStart",
                "SubagentStop",
                "Stop",
                "SessionEnd",
                "SessionStart",
            ] {
                let mut event = event(name);
                if !matches!(name, "SubagentStart" | "SubagentStop") {
                    event.agent_id = Some("worker".into());
                    event.source = Some("startup".into());
                }
                assert!(matches!(hook_outcome(kind, &event), HookOutcome::Nothing));
                let actions = crate::input::actions::hook_report_actions(
                    kind,
                    crate::pane_id::PaneId::Terminal(crate::internal_ids::TerminalId::new(9)),
                    &event,
                    serde_json::to_string(&event).unwrap(),
                );
                assert!(
                    matches!(actions.as_slice(), [crate::input::actions::Action::AgentEvent(event)]
                    if event.hook == name)
                );
            }
        }
    }

    #[test]
    fn the_hook_settings_wire_every_event_to_the_command() {
        let settings = claude_hook_settings("yinz action agent-report --hook-json");
        let hooks = settings["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), CLAUDE_HOOK_EVENTS.len());
        for event in CLAUDE_HOOK_EVENTS {
            let handler = &hooks[*event][0]["hooks"][0];
            assert_eq!(handler["type"], "command", "{event}");
            assert_eq!(handler["command"], "yinz action agent-report --hook-json");
            assert_eq!(handler["timeout"], CLAUDE_HOOK_TIMEOUT_SECONDS);
            assert!(
                hooks[*event][0].get("matcher").is_none(),
                "{event} fires on every occurrence"
            );
        }

        let command = "yinz action agent-report --kind codex --hook-json";
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
    fn the_muse_settings_wire_its_events_and_not_the_llm_firehose() {
        let command = "/root/.config/muse/yinz/yinz-hook.sh";
        let settings = muse_hook_settings(command);
        let hooks = settings["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), MUSE_HOOK_EVENTS.len());
        for event in MUSE_HOOK_EVENTS {
            let handler = &hooks[*event][0]["hooks"][0];
            assert_eq!(handler["type"], "command", "{event}");
            assert_eq!(handler["command"], command, "{event}");
            assert_eq!(handler["timeout"], MUSE_HOOK_TIMEOUT_SECONDS, "{event}");
            assert!(
                hooks[*event][0].get("matcher").is_none(),
                "{event} fires on every occurrence"
            );
        }
        assert!(hooks.get("Notification").is_some());
        assert!(hooks.get("PreLLMCall").is_none());
        assert!(hooks.get("PostLLMCall").is_none());
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

    #[test]
    fn a_agent_launcher_is_looked_through_for_the_agents_name() {
        let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        for launcher in ["codex", "muse"] {
            assert_eq!(
                agent_kind_of(
                    Path::new("yinz"),
                    &args(&[
                        "agent",
                        launcher,
                        "--command",
                        launcher,
                        "--",
                        "resume",
                        "--last"
                    ])
                )
                .as_deref(),
                Some(launcher)
            );
            // The bare form runs the launcher's own default program.
            assert_eq!(
                agent_kind_of(
                    Path::new("yinz"),
                    &args(&["agent", launcher, "--", "--model", "x"])
                )
                .as_deref(),
                Some(launcher)
            );
            // Anything else under `agent` is maintenance, not an agent pane.
            assert_eq!(
                agent_kind_of(Path::new("yinz"), &args(&["agent", "hooks", "install"])).as_deref(),
                Some("yinz")
            );
        }
    }

    #[test]
    fn mother_fallback_preserves_agent_detection_without_interpreting_arbitrary_shell() {
        let mut args: Vec<String> = [
            "-c",
            MOTHER_FALLBACK_SCRIPT,
            "mother",
            "setup failed",
            "claude",
            "--resume",
            "conversation",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            agent_kind_of(Path::new("/bin/sh"), &args).as_deref(),
            Some("claude")
        );
        assert_eq!(
            kind_from_program_and_args(Path::new("/bin/sh"), &args).as_deref(),
            Some("claude")
        );
        args[1] = "echo claude".into();
        assert_eq!(
            agent_kind_of(Path::new("/bin/sh"), &args).as_deref(),
            Some("sh")
        );
        assert!(kind_from_program_and_args(Path::new("/bin/sh"), &args).is_none());
    }

    #[test]
    fn palette_launchers_keep_provider_identity_with_default_and_explicit_commands() {
        for (launcher, provider) in [("claude", "claude"), ("codex-untracked", "codex")] {
            for arguments in [
                vec!["agent", launcher, "--", "prompt"],
                vec!["agent", launcher, "--command", provider, "--", "prompt"],
            ] {
                let args: Vec<String> = arguments.into_iter().map(str::to_owned).collect();
                assert_eq!(
                    kind_from_program_and_args(Path::new("yinz"), &args).as_deref(),
                    Some(provider)
                );
            }
        }
    }

    #[test]
    fn mother_fallback_round_trip_and_exact_recognition() {
        let mut program = PathBuf::from("env");
        let mut args = vec!["SETTING=value".into(), "codex".into(), "--resume".into()];
        mother_fallback(&mut program, &mut args, "failed\n\x1b[31mcheck");
        assert_eq!(mother_fallback_command_index(&program, &args), Some(4));
        assert!(!args[3].contains(['\n', '\x1b']));
        let (inner, tail) = unwrapped_program_and_args(&program, &args).unwrap();
        assert_eq!(inner, Path::new("codex"));
        assert_eq!(tail, ["--resume"]);
        for index in [0, 1, 2] {
            let mut malformed = args.clone();
            malformed[index] = "other".into();
            assert_eq!(mother_fallback_command_index(&program, &malformed), None);
        }
        assert_eq!(mother_fallback_command_index(&program, &args[..4]), None);
        assert_eq!(
            mother_fallback_command_index(Path::new("bash"), &args),
            None
        );
    }

    #[test]
    fn a_sandboxed_pane_reports_the_command_inside_its_launcher_or_bwrap() {
        let args = |words: &[&str]| -> Vec<String> {
            words.iter().map(|word| (*word).to_owned()).collect()
        };
        let inner = args(&["env", "YINZ_AGENT_PANE=1", "claude", "--", "prompt"]);
        let behind = |prefix: &[&str]| [args(prefix), inner.clone()].concat();
        for argv in [
            // The launcher's own name is whatever this binary was installed as.
            behind(&["/opt/yinz-dev", BUBBLEWRAP_RUN_FLAG, "/project", "--"]),
            behind(&["bwrap", "--unshare-all", "--chdir", "/project", "--"]),
            behind(&["/usr/bin/bwrap", "--"]),
        ] {
            assert_eq!(bubblewrap_inner_argv(&argv), Some(inner.as_slice()));
        }
        for argv in [
            args(&["claude", "--", "prompt"]),
            args(&["bwrap", "--unshare-all"]),
            args(&["bwrap", "--"]),
            args(&["yinz", BUBBLEWRAP_RUN_FLAG, "/project", "claude"]),
        ] {
            assert_eq!(bubblewrap_inner_argv(&argv), None, "{argv:?}");
        }
    }

    #[test]
    fn a_command_line_names_an_agent_by_its_first_word_or_behind_an_interpreter() {
        for (command, kind) in [
            ("claude", "claude"),
            ("claude-code --resume", "claude"),
            ("/usr/local/bin/Codex --resume", "codex"),
            ("/Users/x/.local/bin/muse-bin-0.1.0-R708.1 --resume", "muse"),
            ("node /opt/bin/gemini", "gemini"),
            ("pi-agent", "pi"),
            ("yinz agent codex --command codex -- resume --last", "codex"),
            ("yinz agent muse --command muse -- resume --last", "muse"),
        ] {
            assert_eq!(
                kind_from_command(command).as_deref(),
                Some(kind),
                "{command}"
            );
        }
        for command in [
            "zsh",
            "pipenv run pi",
            "git commit -m 'ask claude'",
            "yinz hooks install",
            "",
        ] {
            assert_eq!(kind_from_command(command), None, "{command}");
        }
    }

    #[test]
    fn a_program_and_its_arguments_name_an_agent_through_env_and_an_interpreter() {
        let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            kind_from_program_and_args(Path::new("/usr/bin/claude"), &args(&["--model", "opus"]))
                .as_deref(),
            Some("claude")
        );
        assert_eq!(
            kind_from_program_and_args(Path::new("env"), &args(&["FOO=1", "codex", "--resume"]))
                .as_deref(),
            Some("codex")
        );
        assert_eq!(
            kind_from_program_and_args(
                Path::new("/usr/bin/env"),
                &args(&["node", "/opt/bin/gemini"])
            )
            .as_deref(),
            Some("gemini")
        );
        for (list, kind) in [
            (&["-C", "/repo", "node", "/opt/bin/claude"][..], "claude"),
            (&["-u", "HOME", "node", "/opt/bin/claude"][..], "claude"),
            (
                &["--chdir", "/repo", "python3", "-u", "/opt/bin/codex"][..],
                "codex",
            ),
            (
                &["--unset", "HOME", "node", "/opt/bin/gemini"][..],
                "gemini",
            ),
        ] {
            assert_eq!(
                kind_from_program_and_args(Path::new("env"), &args(list)).as_deref(),
                Some(kind),
                "env {list:?}"
            );
        }
        for (program_path, list) in [
            (
                "node",
                &["--max-old-space-size=4096", "/opt/bin/claude"][..],
            ),
            ("uvx", &["pi"]),
            ("uv", &["run", "pi"]),
            ("bun", &["run", "claude"]),
            ("deno", &["run", "--allow-all", "/opt/bin/claude.js"]),
        ] {
            assert!(
                kind_from_program_and_args(Path::new(program_path), &args(list)).is_some(),
                "{program_path} {list:?}"
            );
        }
        // Only the script counts behind an interpreter: this decides where a pane goes.
        for (program_path, list) in [
            ("python3", &["gemini_test.py", "--api", "gemini"][..]),
            ("node", &["build.js", "--preset", "claude-3"]),
            ("uv", &["run", "x", "--with", "pi"]),
            ("uv", &["--directory", "/work/claude", "run", "pytest"]),
            ("uv", &["--directory", "/work/project", "run", "claude"]),
            ("uv", &["run", "--with", "claude", "pytest"]),
            ("uv", &["--directory"]),
            ("uv", &["run", "run", "claude"]),
            ("node", &["--require", "/work/claude", "build.js"]),
            ("node", &["--unknown=value", "claude"]),
            ("node", &["-e", "claude"]),
            ("python3", &["-c", "claude"]),
            ("python3", &["-m", "claude"]),
            ("python3", &["run", "claude"]),
            ("node", &["python3", "claude"]),
            ("node", &["FOO=1", "claude"]),
            ("node", &[]),
            ("/bin/zsh", &["-c", "claude"]),
            ("env", &["FOO=1"]),
        ] {
            assert_eq!(
                kind_from_program_and_args(Path::new(program_path), &args(list)),
                None,
                "{program_path} {list:?}"
            );
        }
    }

    #[test]
    fn codex_bootstrap_marker_survives_pruning_and_provider_hooks_omit_it() {
        for ready in [true, false] {
            let hook = HookEvent {
                codex_hooks_ready: Some(ready),
                ..event("SessionStart")
            };
            let pruned: HookEvent =
                serde_json::from_value(serde_json::to_value(hook.pruned_to(0)).unwrap()).unwrap();
            assert_eq!(pruned.codex_hooks_ready, Some(ready));
        }
        assert!(serde_json::to_value(event("SessionStart"))
            .unwrap()
            .get("_yinz_codex_hooks_ready")
            .is_none());
    }

    #[test]
    fn muse_bootstrap_marker_survives_pruning_and_provider_hooks_omit_it() {
        for ready in [true, false] {
            let hook = HookEvent {
                muse_hooks_ready: Some(ready),
                ..event("SessionStart")
            };
            let pruned: HookEvent =
                serde_json::from_value(serde_json::to_value(hook.pruned_to(0)).unwrap()).unwrap();
            assert_eq!(pruned.muse_hooks_ready, Some(ready));
        }
        assert!(serde_json::to_value(event("SessionStart"))
            .unwrap()
            .get("_yinz_muse_hooks_ready")
            .is_none());
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
        let clear = start("clear");
        assert_eq!(hook_state(&clear), Some(AgentState::Idle));
        let HookOutcome::Reported(agent) = hook_outcome("claude", &clear) else {
            panic!("clear must restore the agent record");
        };
        assert_eq!(agent.state, AgentState::Idle);
        assert_eq!(agent.source, AgentSource::Reported);
        assert_eq!(hook_state(&start("compact")), None);
        assert_eq!(hook_state(&start("unknown")), None);
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
        assert_eq!(hook_state(&notification("permission_prompt")), None);
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

        // Display events carry prose without changing state. SessionEnd ends the record.
        for wired in CLAUDE_HOOK_EVENTS {
            if matches!(
                *wired,
                HOOK_SESSION_END
                    | "Notification"
                    | "MessageDisplay"
                    | "SubagentStart"
                    | "SubagentStop"
            ) {
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

    /// Other providers use the permission mode to distinguish automatic approval.
    /// Direct questions and elicitation still need a person under every mode.
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

        // Direct questions and elicitation notifications are always waits. None is a
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
            for kind in WAITING_NOTIFICATIONS {
                let notification = HookEvent {
                    permission_mode: Some(mode.to_owned()),
                    ..notification(kind)
                };
                assert_eq!(hook_state(&notification), Some(AgentState::Waiting));
            }
            for kind in ["permission_prompt", "worker_permission_prompt"] {
                assert_eq!(hook_state(&notification(kind)), None);
            }
        }
    }

    #[test]
    fn claude_permission_evaluation_stays_busy_until_a_prompt_notification() {
        for mode in [
            None,
            Some("plan"),
            Some("default"),
            Some("acceptEdits"),
            Some("auto"),
            Some("dontAsk"),
            Some("bypassPermissions"),
        ] {
            let request = HookEvent {
                tool_name: Some("Bash".to_owned()),
                tool_input: Some(json!({"command": "git status"})),
                permission_mode: mode.map(str::to_owned),
                ..event("PermissionRequest")
            };
            let HookOutcome::Reported(info) = hook_outcome("claude", &request) else {
                panic!("permission evaluation reports work");
            };
            assert_eq!(info.state, AgentState::Busy);
            assert_eq!(info.detail.as_deref(), Some("Bash git"));
            assert_eq!(info.waiting_on, None);
            assert!(!info.wait_needs_a_person);
            let raw = serde_json::to_string(&request).unwrap();
            let broadcast = AgentEvent::from_hook(
                "claude",
                crate::internal_ids::TerminalId::new(7),
                &request,
                raw.clone(),
            );
            assert_eq!(broadcast.state, Some(info.state));
            assert_eq!(broadcast.detail, info.detail);
            assert_eq!(broadcast.raw, raw);

            for tool in WAITING_TOOLS {
                let asking = HookEvent {
                    tool_name: Some((*tool).to_owned()),
                    ..request.clone()
                };
                assert_eq!(
                    hook_state_for(
                        "claude",
                        &HookEvent {
                            hook_event_name: "PreToolUse".to_owned(),
                            ..asking.clone()
                        }
                    ),
                    Some(AgentState::Waiting)
                );
                assert_eq!(hook_outcome("claude", &asking), HookOutcome::Nothing);
                assert_eq!(
                    hook_state_for(
                        "claude",
                        &HookEvent {
                            hook_event_name: "PostToolUse".to_owned(),
                            ..asking
                        }
                    ),
                    Some(AgentState::Busy)
                );
            }
            for kind in ["permission_prompt", "worker_permission_prompt"] {
                let prompt = HookEvent {
                    permission_mode: mode.map(str::to_owned),
                    message: Some("Allow Bash?".to_owned()),
                    ..notification(kind)
                };
                for worker in [None, Some("worker")] {
                    let prompt = HookEvent {
                        agent_id: worker.map(str::to_owned),
                        ..prompt.clone()
                    };
                    let expected = (kind == "permission_prompt").then_some(AgentState::Waiting);
                    if worker.is_none() && expected.is_some() {
                        let HookOutcome::Reported(info) = hook_outcome("claude", &prompt) else {
                            panic!("a prompt notification reports waiting");
                        };
                        assert_eq!(info.state, AgentState::Waiting);
                        assert_eq!(info.waiting_on.as_deref(), Some("Allow Bash?"));
                        assert!(info.wait_needs_a_person);
                    } else {
                        assert_eq!(hook_outcome("claude", &prompt), HookOutcome::Nothing);
                    }
                    assert_eq!(hook_state_for("claude", &prompt), expected);
                    for provider in ["codex", "muse", "opencode"] {
                        assert_eq!(hook_state_for(provider, &prompt), None);
                    }
                    let broadcast = AgentEvent::from_hook(
                        "claude",
                        crate::internal_ids::TerminalId::new(7),
                        &prompt,
                        serde_json::to_string(&prompt).unwrap(),
                    );
                    assert_eq!(broadcast.state, expected);
                }
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
        // Permission evaluation keeps the tool hint without announcing a user wait.
        let permission = HookEvent {
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(serde_json::json!({ "command": "rm -rf target" })),
            ..event("PermissionRequest")
        };
        match hook_outcome("claude", &permission) {
            HookOutcome::Reported(info) => {
                assert_eq!(info.state, AgentState::Busy);
                assert_eq!(info.detail.as_deref(), Some("Bash rm"));
                assert_eq!(info.waiting_on, None);
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

    /// A Muse session's feed, as the host parses it: startup is idle, a submitted turn
    /// is busy, a plain `Stop` ends it (Muse has no blocking-hook continuation to wait
    /// out), and the translated approval notification waits on a person. The payloads
    /// are the shapes captured from `muse exec --provider echo`, with the ids blanked.
    #[test]
    fn a_muse_feed_reads_as_the_lifecycle_it_is() {
        let start = HookEvent {
            source: Some("startup".to_owned()),
            ..event("SessionStart")
        };
        match hook_outcome("muse", &start) {
            HookOutcome::Reported(info) => assert_eq!(info.state, AgentState::Idle),
            other => panic!("{other:?}"),
        }
        match hook_outcome("muse", &event("UserPromptSubmit")) {
            HookOutcome::Reported(info) => assert_eq!(info.state, AgentState::Busy),
            other => panic!("{other:?}"),
        }
        // A snake_case tool still names the row; only its argument hint is lost.
        let pre = HookEvent {
            tool_name: Some("read_skill".to_owned()),
            ..event("PreToolUse")
        };
        match hook_outcome("muse", &pre) {
            HookOutcome::Reported(info) => {
                assert_eq!(info.state, AgentState::Busy);
                assert_eq!(info.detail.as_deref(), Some("read_skill"));
            },
            other => panic!("{other:?}"),
        }
        match hook_outcome("muse", &event("Stop")) {
            HookOutcome::Reported(info) => assert_eq!(info.state, AgentState::Idle),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            hook_outcome("muse", &event(HOOK_SESSION_END)),
            HookOutcome::Ended
        );
        // The translated approval notification: a wait, on a person, carrying the
        // sentence Muse would have shown.
        let approval = HookEvent {
            message: Some("Allow `cargo build`?".to_owned()),
            ..event("Elicitation")
        };
        match hook_outcome("muse", &approval) {
            HookOutcome::Reported(info) => {
                assert_eq!(info.state, AgentState::Waiting);
                assert!(info.wait_needs_a_person);
                assert_eq!(info.waiting_on.as_deref(), Some("Allow `cargo build`?"));
            },
            other => panic!("{other:?}"),
        }
    }

    /// The record carries [`waiting_needs_a_person`]'s answer, because the pane that draws the
    /// waiting bell is a long way from the event that decided it - and the event is gone by
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

        // Automatic approval is busy on every surface.
        let accepted = reported(&request("Edit", "acceptEdits"));
        assert_eq!(accepted.state, AgentState::Busy);
        assert_eq!(accepted.waiting_on, None);
        assert!(!accepted.wait_needs_a_person);

        for kind in ["claude", "codex", "muse"] {
            for mode in [None, Some("default"), Some("plan"), Some("unknown-mode")] {
                let event = HookEvent {
                    permission_mode: mode.map(str::to_owned),
                    ..request("Bash", "default")
                };
                let HookOutcome::Reported(info) = hook_outcome(kind, &event) else {
                    panic!("manual permission must report");
                };
                let expected = if kind == "claude" {
                    AgentState::Busy
                } else {
                    AgentState::Waiting
                };
                assert_eq!(info.state, expected);
                assert_eq!(info.wait_needs_a_person, kind != "claude");
            }
        }

        // Claude still evaluates Bash permissions under acceptEdits.
        let blocked = reported(&request("Bash", "acceptEdits"));
        assert_eq!(blocked.state, AgentState::Busy);
        assert!(!blocked.wait_needs_a_person);

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

        let (pruned, _) = write.broadcast(serde_json::to_string(&write).unwrap());
        let json = serde_json::to_string(pruned.as_ref()).expect("a pruned event serializes");
        assert!(
            json.len() <= HOOK_BROADCAST_MAX_BYTES,
            "a pruned event fits the cap, got {}",
            json.len()
        );
        // Every reading still works off the pruned event, which is the whole point of
        // cutting the strings rather than dropping the fields.
        assert_eq!(hook_detail(&pruned).as_deref(), Some("Write main.rs"));
        assert_eq!(hook_state(&pruned), hook_state(&write));
        assert!(pruned
            .last_assistant_message
            .as_ref()
            .is_some_and(|message| message.len() > 8 * 1024));

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

    /// The typed `prompt` and `last_message` are bounded the way `raw` is, and say what it
    /// says: a multi-megabyte final message must not reach every plugin's memory through the
    /// typed field after the prune kept it out of the JSON.
    #[test]
    fn a_typed_events_prose_is_bounded_like_its_raw_json() {
        let huge = "x".repeat(2 * 1024 * 1024);
        let stop = HookEvent {
            last_assistant_message: Some(huge.clone()),
            prompt: Some(huge),
            ..event("Stop")
        };
        let json = serde_json::to_string(&stop).expect("the event serializes");
        let typed = AgentEvent::from_hook(
            "claude",
            crate::internal_ids::TerminalId::new(7),
            &stop,
            json,
        );
        assert!(typed.raw.len() <= HOOK_BROADCAST_MAX_BYTES);
        let arrived: HookEvent = serde_json::from_str(&typed.raw).expect("still an event");
        assert_eq!(typed.last_message, arrived.last_assistant_message);
        assert_eq!(typed.prompt, arrived.prompt);
        assert!(typed
            .last_message
            .as_ref()
            .is_some_and(|message| message.len() > 8 * 1024));
        assert!(typed
            .prompt
            .as_ref()
            .is_some_and(|prompt| prompt.len() > 8 * 1024));

        // Under the cap, whole - nearly every event.
        let small = HookEvent {
            last_assistant_message: Some("Ran the tests; they pass.".to_owned()),
            ..event("Stop")
        };
        let json = serde_json::to_string(&small).expect("the event serializes");
        let typed = AgentEvent::from_hook(
            "claude",
            crate::internal_ids::TerminalId::new(7),
            &small,
            json.clone(),
        );
        assert_eq!(typed.raw, json);
        assert_eq!(typed.last_message, small.last_assistant_message);
        assert_eq!(typed.prompt, None);
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

    #[test]
    fn hook_json_boundaries_and_full_prompt_identity() {
        for size in [
            HOOK_BROADCAST_MAX_BYTES - 1,
            HOOK_BROADCAST_MAX_BYTES,
            HOOK_BROADCAST_MAX_BYTES + 1,
        ] {
            let mut value =
                json!({"hook_event_name":"UserPromptSubmit", "prompt":"", "turn_id":"turn-1"});
            let overhead = value.to_string().len();
            value["prompt"] = Value::String("x".repeat(size - overhead));
            let raw = value.to_string();
            assert_eq!(raw.len(), size);
            let hook: HookEvent = serde_json::from_str(&raw).unwrap();
            let (_, bounded) = hook.broadcast(raw.clone());
            assert!(bounded.len() <= HOOK_BROADCAST_MAX_BYTES);
            if size <= HOOK_BROADCAST_MAX_BYTES {
                assert_eq!(bounded, raw);
            }
        }
        let metadata = HookEvent {
            session_id: "\u{0001}".repeat(HOOK_BROADCAST_MAX_BYTES),
            ..event("Stop")
        };
        assert!(
            metadata
                .broadcast_json(serde_json::to_string(&metadata).unwrap())
                .len()
                <= HOOK_BROADCAST_MAX_BYTES
        );
        for text in ["x", "計🙂", "\\\"\n\u{0001}"] {
            let prompt = text.repeat(HOOK_BROADCAST_MAX_BYTES);
            let hook = HookEvent {
                prompt: Some(prompt.clone()),
                turn_id: Some("turn-1".into()),
                prompt_id: Some("prompt-1".into()),
                claude_turn_complete: Some(true),
                claude_background_tasks: (0..40).map(|task| format!("task-{task}")).collect(),
                agy_turn_complete: Some(true),
                message: Some(prompt.clone()),
                last_assistant_message: Some(prompt.clone()),
                tool_input: Some(json!({"plan":prompt, "content":"x".repeat(300000)})),
                ..event("UserPromptSubmit")
            };
            let (bounded, raw) = hook.broadcast(serde_json::to_string(&hook).unwrap());
            assert!(raw.len() <= HOOK_BROADCAST_MAX_BYTES);
            assert!(bounded.prompt.as_ref().unwrap().len() > 8192);
            assert_eq!(bounded.turn_id, hook.turn_id);
            assert_eq!(bounded.prompt_id, hook.prompt_id);
            assert_eq!(bounded.claude_turn_complete, hook.claude_turn_complete);
            assert_eq!(
                bounded.claude_background_tasks,
                hook.claude_background_tasks[..MAX_BACKGROUND_TASKS]
            );
            assert_eq!(bounded.agy_turn_complete, hook.agy_turn_complete);
            assert_eq!(
                bounded.prompt_sha256,
                Some(crate::agent::control::prompt_sha256(&prompt))
            );
            let arrived: HookEvent = serde_json::from_str(&raw).unwrap();
            assert_eq!(&arrived, bounded.as_ref());
            // A remote broker can prune before forwarding to the host.
            // A second prune must keep the digest of the original, complete prompt.
            let mut forwarded: Value = serde_json::from_str(&raw).unwrap();
            forwarded["message"] = Value::String("message".repeat(1000));
            let forwarded_hook: HookEvent = serde_json::from_value(forwarded.clone()).unwrap();
            let (repruned, forwarded_raw) = forwarded_hook.broadcast(forwarded.to_string());
            assert!(forwarded_raw.len() <= HOOK_BROADCAST_MAX_BYTES);
            assert_eq!(repruned.prompt_sha256, bounded.prompt_sha256);
            assert_eq!(repruned.turn_id, bounded.turn_id);
            let budget = bounded.prompt.as_ref().unwrap().len();
            assert!(
                serde_json::to_string(
                    &HookEvent {
                        prompt_sha256: bounded.prompt_sha256.clone(),
                        ..hook
                    }
                    .pruned_to(budget + 4)
                )
                .unwrap()
                .len()
                    > HOOK_BROADCAST_MAX_BYTES
            );
        }
    }

    /// A cut would drop a collapsed paste's closer and leave its opener in the prompt.
    #[test]
    fn an_oversize_collapsed_paste_is_unwrapped_before_it_is_cut() {
        let body = "x\n".repeat(HOOK_BROADCAST_MAX_BYTES);
        let prompt = format!(
            "{} \n\n<pasted_content id=\"8c9e\">\n{body}\n</pasted_content id=\"8c9e\">\n",
            crate::agent::control::PASTE_LEAD_IN
        );
        let hook = HookEvent {
            prompt: Some(prompt.clone()),
            ..event("UserPromptSubmit")
        };
        let (bounded, raw) = hook.broadcast(serde_json::to_string(&hook).unwrap());
        assert!(raw.len() <= HOOK_BROADCAST_MAX_BYTES);
        let kept = bounded.prompt.as_deref().unwrap();
        assert!(!kept.contains("pasted_content"));
        let submitted = crate::agent::control::submitted_prompt(kept);
        assert!(submitted.len() > 8192);
        assert!(body.starts_with(submitted.as_ref()));
        assert_eq!(
            bounded.prompt_sha256,
            Some(crate::agent::control::prompt_sha256(&prompt))
        );
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
        let json = event.broadcast_json(serde_json::to_string(&event).expect("event serializes"));
        assert!(json.len() <= HOOK_BROADCAST_MAX_BYTES, "{}", json.len());
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

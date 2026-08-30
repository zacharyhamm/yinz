//! The tool set the model uses to drive Zellij.
//!
//! Split in two halves. Everything above the dispatch section is pure and
//! host-testable; `dispatch` and its helpers call the Zellij host and are not.
//! That split is load-bearing rather than stylistic: `host_run_plugin_command`
//! is a no-op on non-wasm32 targets, and most value-returning shims `.unwrap()`
//! the protobuf decode, so calling one from a host-triple test panics.

// Most of the pure half exists for `mod host`, which only compiles on wasm32 - so linting this
// crate for the host triple (as a workspace-wide `cargo clippy` does) reads those helpers as dead.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::BTreeMap;

use hocus_proto::launch;
use hocus_proto::location::Location;
use hocus_proto::paths;
use hocus_proto::projects::{self, Project};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zellij_tile::prelude::{
    FloatingPaneCoordinates, GitCheckout, PaneId, PercentOrFixed, PermissionType, SessionInfo,
    WarpPlacement,
};

use crate::log::truncate_str;
use crate::lua_docs;
use crate::MAX_TOOL_OUTPUT;

/// One complete call requested by the model, reassembled from stream fragments.
///
/// Serialized into the archive along with the rest of the transcript: a chat
/// saved mid-tool-call must round-trip, or continuing it sends the API an
/// assistant turn whose calls have no results.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON, exactly as the model emitted it. Parsed at dispatch, not here:
    /// a malformed object should reach the model as a tool result it can retry
    /// from, not vanish during accumulation.
    pub arguments: String,
}

/// Whether this call — arguments and all — stops for a y/n.
///
/// `needs_confirmation` answers for the tools that always stop; a couple stop
/// only in one shape, and the shape is in the arguments. That decision lives
/// here, next to `consequence`, because the two have to agree: a call that stops
/// needs a sentence explaining it, and a sentence is only ever shown for a call
/// that stops.
pub fn call_needs_confirmation(call: &ToolCall) -> bool {
    match call.name.as_str() {
        // Opening a bare pane is a shell the user drives, like `zellij action
        // new-pane`, and does not ask. Naming a `command` up front is the model
        // choosing what runs on this machine — that is the confirmable part, and
        // leaving it unconfirmed is a prompt-injection path to local execution.
        // A login shell on another machine is the same shape one machine along:
        // it runs nothing the model chose, and making it do something still goes
        // through `send_input`, which asks. Naming a command up front is the
        // confirmable part on either machine.
        "new_pane" | "new_remote_pane" => names_a_command(call),
        // Scheduling is otherwise free; handing a job standing permission to use
        // tools without asking again is the confirmable part.
        "schedule" => Args::parse(&call.arguments).is_ok_and(|args| args.auto_confirm),
        name => needs_confirmation(name),
    }
}

/// True when a `new_pane`/`new_remote_pane` call names a program to run rather
/// than opening a bare shell — the same reading the dispatchers use, through
/// `Args::program_and_args`, so what is confirmed is what runs.
///
/// The `unwrap_or(true)` is load-bearing: defaulted `Args` have no command
/// either, so reading a malformed call as a bare shell would let it through
/// without a prompt — the one case `consequence`'s doc singles out as needing
/// one most. Arguments we cannot read might name any command, so the safe
/// reading is "this runs something". A well-formed call with no command is the
/// one shape that does not stop.
fn names_a_command(call: &ToolCall) -> bool {
    Args::parse(&call.arguments)
        .map(|args| args.program_and_args().is_some())
        .unwrap_or(true)
}

/// Tools that need explicit confirmation before dispatch whatever they are
/// given. `call_needs_confirmation` is the question the tool loop actually asks;
/// this is the part of the answer that does not depend on the arguments.
pub fn needs_confirmation(name: &str) -> bool {
    matches!(
        name,
        "rerun_pane"
            | "clear_pane"
            | "send_input"
            | "agent_send"
            | "agent_control"
            | "agent_spawn"
            // The one tool that removes something the user meant to keep.
            | "project_forget"
            | "slack_post"
            // Moving a pane is disruptive in a way the user cannot undo by
            // looking away: the pane leaves the screen, and for a moment during
            // the handoff it belongs to no session at all.
            | "warp_pane"
            | "warp_pane_here"
            // The tab version of the above, with more at stake: every pane in
            // the tab leaves at once, and warping a session's last tab ends it.
            | "warp_tab"
            | "warp_tab_here"
            | "adopt_orphaned_pane"
            // Putting code the model wrote into the user's config dir. `plugin_reload` is not
            // here: it re-runs a file approved one step earlier, and plugin-manager already
            // reloads a plugin on a keypress - so confirming both would cost two prompts per
            // write-reload-fix iteration to say the same thing twice.
            | "plugin_write"
            | "skill_write"
            | "skill_enable"
            | "skill_disable"
    )
}

/// The one-line transcript label for a call. Arguments are shown as the model
/// wrote them, so a confirmation prompt says exactly what is about to happen.
///
/// Source-writing tools are the exception: their arguments carry a whole file, and showing them as
/// written would paste the plugin into the conversation on the line that says it is being
/// written. The source is still shown, in `confirmation_detail`, where there is room for it.
pub fn summarize(call: &ToolCall) -> String {
    let arguments = call.arguments.trim();
    if matches!(call.name.as_str(), "plugin_write" | "skill_write") {
        let args = Args::parse(&call.arguments).unwrap_or_default();
        let name = args.name.as_deref().unwrap_or("?");
        let lines = args
            .source
            .as_deref()
            .map(|s| s.lines().count())
            .unwrap_or(0);
        return format!("{} {name} ({lines} lines)", call.name);
    }
    if arguments.is_empty() || arguments == "{}" {
        call.name.clone()
    } else {
        format!("{} {arguments}", call.name)
    }
}

/// What running `call` will actually do, said in the user's terms rather than the
/// model's.
///
/// Deliberately not the tool's own description from `specs()`: those are written
/// to tell a model what a tool is *for* ("Send text, then keys, to a running
/// program"), and the question here is what is about to happen to this session
/// if you press `y`. `None` for a tool that never asks.
///
/// Arguments that fail to parse fall back to the bare effect. A confirmation for
/// a call whose arguments are malformed is exactly when the user most needs to
/// see something, so this must not go quiet.
pub fn consequence(call: &ToolCall) -> Option<String> {
    let args = Args::parse(&call.arguments).unwrap_or_default();
    let pane = match args.pane_id {
        Some(pane_id) => format!("pane {pane_id}"),
        None => "a pane".to_owned(),
    };
    let sentence = match call.name.as_str() {
        "rerun_pane" => format!("re-runs {pane}, killing whatever it is running now"),
        "clear_pane" => format!("throws away {pane}'s scrollback; it cannot be scrolled back"),
        "send_input" => format!("types into {pane}, as if you had typed it yourself"),
        "agent_send" => format!("types into the agent in {pane}, as if you had typed it yourself"),
        "agent_control" => match args.action.as_deref() {
            Some("interrupt") => format!("interrupts the agent in {pane} mid-run"),
            Some("restart") => format!("restarts the agent in {pane}, losing its conversation"),
            Some("close") => format!("closes {pane} and the agent running in it"),
            _ => format!("interrupts, restarts or closes the agent in {pane}"),
        },
        "agent_spawn" => {
            // Read off the plan, never re-derived: the sentence has to promise what
            // dispatch will do, and a default spelled out a second time here is a
            // default that can come to differ from the one that runs.
            let Ok((command, checkout)) = launch::plan(&args.launch_spec()) else {
                // A call `launch` refuses opens nothing, but the prompt still goes
                // up — the same rule every other arm follows for arguments it
                // cannot read.
                return Some("opens a new pane running an agent".to_owned());
            };
            let mut sentence = format!("opens a new pane running `{}`", command.path.display());
            // The one flag worth a sentence of its own: an agent that never asks.
            if matches!(
                args.permission_mode.as_deref().map(str::trim),
                Some("skip" | "bypassPermissions" | "force" | "full-auto" | "never")
            ) {
                sentence.push_str(" with its permission checks off");
            }
            // Named in full, because this is the half that writes to the user's
            // repository and a confirmation that hid it would be worth nothing.
            match checkout {
                GitCheckout::NewWorktree { base, path, .. } => sentence.push_str(&format!(
                    ", on a new worktree `{}` branched from `{base}`",
                    path.display()
                )),
                GitCheckout::NewBranch { branch, base } => sentence.push_str(&format!(
                    ", after switching this checkout to a new branch `{branch}` off `{base}`"
                )),
                GitCheckout::Current => {},
            }
            sentence
        },
        "project_forget" => {
            let name = args.name.as_deref().unwrap_or("a project");
            format!("forgets the project {name}; its todos stay, the project does not")
        },
        "slack_post" => {
            let channel = args.channel.as_deref().unwrap_or("a channel");
            match args.thread.as_deref() {
                Some(_) => {
                    format!("posts to {channel} as a thread reply, visible to everyone in it")
                },
                None => format!("posts to {channel} immediately, visible to everyone in it"),
            }
        },
        "warp_pane" => {
            let session = args.to_session.as_deref().unwrap_or("another session");
            format!("moves {pane} out to {session}; it leaves this screen")
        },
        "warp_pane_here" => {
            let session = args.from_session.as_deref().unwrap_or("another session");
            format!("takes {pane} from {session} into this one")
        },
        "warp_tab" => {
            let session = args.to_session.as_deref().unwrap_or("another session");
            format!("moves a whole tab out to {session}; if it is the last tab, this session ends")
        },
        "warp_tab_here" => {
            let session = args.from_session.as_deref().unwrap_or("another session");
            format!("takes a tab from {session} into this one")
        },
        "adopt_orphaned_pane" => "takes an orphaned pane into this session".to_owned(),
        // Only the shape that stops: a bare pane (a shell, no command) is not
        // confirmed, so it has nothing to say here. See `call_needs_confirmation`.
        "new_pane" if names_a_command(call) => match args.program_and_args() {
            Some((program, _)) => format!("opens a new pane running `{program}` on this machine"),
            // Arguments that would not parse still stop (see `names_a_command`);
            // the prompt goes up without the program name rather than going quiet.
            None => "opens a new pane running a command on this machine".to_owned(),
        },
        // Only the shape that stops: a bare login shell is not confirmed, so it
        // has nothing to say here. See `call_needs_confirmation`.
        "new_remote_pane" if names_a_command(call) => {
            let host = args.remote_host.as_deref().unwrap_or("another machine");
            format!("runs a command on {host} over ssh, opening a new pane connected to it")
        },
        // Likewise: `pump_tools` stops for exactly the standing-permission case.
        "schedule" if args.auto_confirm => {
            "lets every run of this job use tools without stopping to ask again".to_owned()
        },
        // The authoring act: an LLM putting code into the user's config dir. It is inert until
        // something loads it, which is why the write is the prompt and the reload is not.
        "plugin_write" => {
            let name = args.name.as_deref().unwrap_or("a plugin");
            let mut sentence = match args.overwrite {
                true => format!("replaces {name}.lua in your Zellij plugin folder"),
                false => format!("writes {name}.lua into your Zellij plugin folder"),
            };
            // The approval of the source is the approval of what it asks for: saying yes here
            // is what grants these, so they have to be in the sentence being said yes to.
            let granted = requested_permissions(args.source.as_deref().unwrap_or(""));
            if !granted.is_empty() {
                sentence.push_str(&format!(
                    ", and grants it: {}",
                    permission_names(&granted).join(", ")
                ));
            }
            sentence
        },
        "skill_write" => {
            let name = args.name.as_deref().unwrap_or("a skill");
            let scope = args.scope.as_deref().unwrap_or("user");
            match args.overwrite {
                true => format!("replaces the {scope} skill {name}"),
                false => format!("writes the {scope} skill {name}"),
            }
        },
        "skill_enable" | "skill_disable" => {
            let name = args.name.as_deref().unwrap_or("a skill");
            let scope = args.scope.as_deref().unwrap_or("a tier");
            let action = call.name.trim_start_matches("skill_");
            format!("{action}s the {scope} skill {name}")
        },
        _ => return None,
    };
    Some(sentence)
}

/// The permissions a plugin's source asks for in `zellij.request_permission { ... }`, so that
/// approving the write can grant them and the plugin never prompts in its own pane - where the
/// model, driving the write-reload-fix loop, cannot see or answer it.
///
/// Read lexically, not by running the Lua: every quoted string inside the braces of a
/// `request_permission` call, in either spelling (`"read_files"` or `"ReadFiles"`), which is
/// what the prelude accepts. A name zellij does not know is dropped rather than refused, since
/// the plugin's own call will report it. Anything more elaborate - a permission list built at
/// runtime - gets the prompt it always got.
pub fn requested_permissions(source: &str) -> Vec<PermissionType> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("request_permission") {
        rest = &rest[at + "request_permission".len()..];
        let Some(open) = rest.find(['{', '(']) else {
            break;
        };
        // Only whitespace may sit between the name and its argument, or this is some other
        // identifier that happens to end the same way.
        if !rest[..open].trim().is_empty() {
            continue;
        }
        let closer = if rest.as_bytes()[open] == b'{' {
            '}'
        } else {
            ')'
        };
        let Some(close) = rest[open..].find(closer) else {
            break;
        };
        let inside = &rest[open + 1..open + close];
        for quoted in inside.split(['"', '\'']).skip(1).step_by(2) {
            let pascal = quoted
                .split('_')
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<String>();
            if let Ok(permission) = pascal.parse::<PermissionType>() {
                if !found.contains(&permission) {
                    found.push(permission);
                }
            }
        }
        rest = &rest[open + close..];
    }
    found
}

/// Permissions as a plugin author spells them: `read_files`, not `ReadFiles`.
pub fn permission_names(permissions: &[PermissionType]) -> Vec<String> {
    permissions
        .iter()
        .map(|permission| {
            let mut out = String::new();
            for (i, c) in format!("{permission:?}").chars().enumerate() {
                if c.is_uppercase() && i > 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            }
            out
        })
        .collect()
}

/// How many lines of source a toast keeps before summarizing the rest.
const SOURCE_PREVIEW_LINES: usize = 20;

/// The call's arguments as one `key: value` line each, so the thing being agreed
/// to can be read rather than parsed.
///
/// Keys come out alphabetically: the workspace does not enable serde_json's
/// `preserve_order`, so an object is a `BTreeMap`. Arbitrary but stable, which
/// is what a prompt needs.
///
/// Strings lose their quotes; everything else is compact JSON. Arguments that do
/// not parse as an object are handed back verbatim, for the same reason
/// `consequence` does not go quiet on them.
fn argument_lines_with_source_limit(call: &ToolCall, source_limit: Option<usize>) -> Vec<String> {
    let arguments = call.arguments.trim();
    if arguments.is_empty() || arguments == "{}" {
        return Vec::new();
    }
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(arguments) else {
        return vec![arguments.to_owned()];
    };
    fields
        .iter()
        .flat_map(|(key, value)| match value {
            // Code and other multiline values read as a block. `split` keeps a trailing empty
            // row, which matters for inputs where the final newline is part of what will run.
            Value::String(text) if key == "source" || text.contains('\n') => {
                let mut lines = vec![format!("{key}:")];
                let mut text_lines = text.split('\n');
                lines.extend(
                    text_lines
                        .by_ref()
                        .take(
                            source_limit
                                .filter(|_| key == "source")
                                .unwrap_or(usize::MAX),
                        )
                        .map(|line| format!("  {line}")),
                );
                let hidden = text_lines.count();
                if hidden > 0 {
                    lines.push(format!("  … {hidden} more lines"));
                }
                lines
            },
            Value::String(text) => vec![format!("{key}: {text}")],
            value => vec![format!("{key}: {value}")],
        })
        .collect()
}

pub fn argument_lines(call: &ToolCall) -> Vec<String> {
    argument_lines_with_source_limit(call, None)
}

pub(crate) fn toast_argument_lines(call: &ToolCall) -> Vec<String> {
    argument_lines_with_source_limit(call, Some(SOURCE_PREVIEW_LINES))
}

/// The whole confirmation block: what is being run, what it will do, and what it
/// was given, one line each.
///
/// The pane prompt renders this in full so the user can inspect everything they
/// are agreeing to.
#[cfg(test)]
pub fn confirmation_detail(call: &ToolCall) -> Vec<String> {
    let mut lines = vec![call.name.clone()];
    lines.extend(
        confirmation_body(call)
            .into_iter()
            .map(|(line, _)| format!("  {line}")),
    );
    lines
}

/// The same block without the tool's name, unindented, and with the consequence
/// line flagged.
///
/// The pane prompt titles its box with the name, so repeating it inside would
/// say it twice, and the box supplies the indent that the toast has to draw for
/// itself. The flag is what lets that one line — the sentence saying what the
/// call will actually do — be the only red thing on screen.
pub fn confirmation_body(call: &ToolCall) -> Vec<(String, bool)> {
    let mut lines = Vec::new();
    if let Some(consequence) = consequence(call) {
        lines.push((consequence, true));
    }
    lines.extend(argument_lines(call).into_iter().map(|line| (line, false)));
    lines
}

/// The confirmation as a toast: a bounded preview behind a line saying who is
/// asking, since a toast is read from a pane that has nothing to do with hocus.
pub fn confirmation_toast(call: &ToolCall) -> String {
    let mut lines = vec![TOAST_HEADLINE.to_owned(), call.name.clone()];
    if let Some(consequence) = consequence(call) {
        lines.push(format!("  {consequence}"));
    }
    lines.extend(
        toast_argument_lines(call)
            .into_iter()
            .map(|line| format!("  {line}")),
    );
    lines.join("\n")
}

/// Named rather than inline so the test that reads it back cannot drift from it.
pub const TOAST_HEADLINE: &str = "hocus needs confirmation";

/// The pane a pending confirmation is about to act on, for the ones that name a
/// pane at all.
///
/// The prompt already says "types into pane 7"; this is what lets the session
/// then go and show you pane 7. Only tools that act on a pane *already in this
/// session* qualify — `warp_pane_here`, `warp_tab_here` and
/// `adopt_orphaned_pane` all name something that is somewhere else, and there is
/// nothing local to point at until after they run.
///
/// `None` for arguments that don't parse, matching `consequence`: the confirmation
/// still goes up on a malformed call, it just goes up without a destination.
pub fn confirmation_target(call: &ToolCall) -> Option<PaneId> {
    let args = Args::parse(&call.arguments).ok()?;
    let pane_id = args.pane_id?;
    match call.name.as_str() {
        // The agent tools address a terminal and say so: `agent_host_call`
        // rewrites them into pane-addressed primitives with `is_plugin: false`.
        "agent_send" | "agent_control" => Some(PaneId::Terminal(pane_id)),
        "send_input" | "rerun_pane" | "clear_pane" | "warp_pane" => match args.is_plugin? {
            true => Some(PaneId::Plugin(pane_id)),
            false => Some(PaneId::Terminal(pane_id)),
        },
        _ => None,
    }
}

/// What to write on the destination pane's frame while it is being pointed at.
///
/// The pane wears this in place of its own title, so it has to say who put it
/// there and that an answer is owed — read from the pane, with no sidebar in
/// sight, `send_input?` alone would be a riddle.
pub fn indicator_label(call: &ToolCall) -> String {
    format!("hocus: {}?", call.name)
}

/// Every advertised tool name, for the manual `/tool` prompt and its completion.
pub fn names() -> Vec<String> {
    specs()
        .as_array()
        .expect("specs is an array")
        .iter()
        .filter_map(|spec| spec["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

/// A slash line typed by the user — `/read_pane pane_id=3 is_plugin=false` — as
/// the `(name, arguments)` pair a `ToolCall` is built from.
///
/// Values go through `serde_json`, falling back to a bare string, so `pane_id=3`
/// is a number, `is_plugin=false` a bool, `panes=[…]` an array and `name=logs` a
/// string. A rest starting with `{` is taken as the argument object verbatim,
/// which is the escape hatch for what pairs cannot express — a value with spaces,
/// a nested object. The name itself is not checked here: `dispatch` already
/// answers `no such tool`.
pub fn parse_command(line: &str) -> Result<(String, String), String> {
    let line = line.trim().trim_start_matches('/');
    let (name, rest) = match line.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (line, ""),
    };
    if name.is_empty() {
        return Err("a tool name is required".to_owned());
    }
    if rest.is_empty() {
        return Ok((name.to_owned(), "{}".to_owned()));
    }
    if rest.starts_with('{') {
        return Ok((name.to_owned(), rest.to_owned()));
    }

    let mut arguments = serde_json::Map::new();
    for pair in rest.split_whitespace() {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!(
                "`{pair}` is not key=value; arguments are key=value pairs, or one JSON object"
            ));
        };
        arguments.insert(
            key.to_owned(),
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned())),
        );
    }
    Ok((name.to_owned(), Value::Object(arguments).to_string()))
}

// ---------------------------------------------------------------------------
// Specs
// ---------------------------------------------------------------------------

/// A pane is addressed by the `(id, is_plugin)` pair Zellij uses, because `id`
/// alone is only unique within its kind. Repeated in several schemas rather
/// than factored into a `$ref`: not every provider resolves references.
fn pane_properties() -> Value {
    json!({
        "pane_id": { "type": "integer" },
        "is_plugin": {
            "type": "boolean",
            "description": "Pane kind; IDs overlap by kind.",
        },
    })
}

/// What can be typed at a pane: literal text, named keys, or both. Shared by
/// `send_input` and `agent_send`, which take the same input and differ only in
/// what they will point it at.
fn input_properties() -> Value {
    json!({
        "text": { "type": "string" },
        "keys": {
            "type": "array",
            "items": {
                "type": "string",
                "enum": [
                    "enter", "tab", "escape", "backspace", "delete", "insert",
                    "up", "down", "left", "right", "home", "end", "page_up",
                    "page_down", "ctrl_a", "ctrl_b", "ctrl_c", "ctrl_d", "ctrl_e",
                    "ctrl_f", "ctrl_k", "ctrl_l", "ctrl_u", "ctrl_w", "ctrl_y", "ctrl_z"
                ],
            },
        },
    })
}

/// Where a warped pane lands in the session receiving it. A closed set, so a
/// placement the model invents is rejected by the schema rather than at dispatch.
fn placement_property() -> Value {
    json!({
        "type": "string",
        "enum": ["new_tab", "focused_tab", "floating"],
        "description": "Defaults to new_tab.",
    })
}

/// How the Slack tools address a conversation and a span of days. Shared by
/// `slack_read` and `slack_backfill`, which take the same window and differ only
/// in whether they read the archive or fill it.
fn slack_properties() -> Value {
    json!({
        "channel": { "type": "string", "description": "Channel name or id; DMs use ids." },
        "workspace": { "type": "string", "description": "Omit for the only workspace." },
        "from": { "type": "string", "description": "Inclusive YYYY-MM-DD." },
        "to": { "type": "string", "description": "Inclusive YYYY-MM-DD." },
    })
}

/// The `tools` array sent with every request.
pub fn specs() -> Value {
    let pane = pane_properties();
    let input = input_properties();
    let slack = slack_properties();
    let placement = placement_property();
    let pane_ref = json!({
        "type": "object",
        "properties": pane_properties(),
        "required": ["pane_id", "is_plugin"],
    });
    json!([
        tool(
            "list_session",
            "List current tabs, panes, and IDs.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "list_other_sessions",
            "List other sessions and their terminal-pane IDs.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "warp_pane",
            "Move a terminal pane to another session; not plugins or the last pane.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "to_session": { "type": "string" },
                    "placement": placement.clone(),
                },
                "required": ["pane_id", "is_plugin", "to_session"],
            }),
        ),
        tool(
            "warp_pane_here",
            "Bring in a terminal pane listed by list_other_sessions.",
            json!({
                "type": "object",
                "properties": {
                    "from_session": { "type": "string" },
                    "pane_id": { "type": "integer" },
                    "placement": placement.clone(),
                },
                "required": ["from_session", "pane_id"],
            }),
        ),
        tool(
            "warp_tab",
            "Move a tab from the current session, as listed by list_session, to another \
             session; defaults to the focused tab. Can move the last tab, ending this session.",
            json!({
                "type": "object",
                "properties": {
                    "tab_id": { "type": "integer", "description": "From list_session; defaults to the focused tab." },
                    "to_session": { "type": "string" },
                },
                "required": ["to_session"],
            }),
        ),
        tool(
            "warp_tab_here",
            "Bring a tab from another session, as listed by list_other_sessions, into the \
             current session. Use this for requests phrased from another session or into here.",
            json!({
                "type": "object",
                "properties": {
                    "from_session": { "type": "string" },
                    "tab_id": { "type": "integer", "description": "The tab's tab_id in that session." },
                },
                "required": ["from_session", "tab_id"],
            }),
        ),
        tool(
            "list_orphaned_panes",
            "List panes orphaned by a dead session.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "adopt_orphaned_pane",
            "Recover a pane from list_orphaned_panes.",
            json!({
                "type": "object",
                "properties": {
                    "host_id": { "type": "string" },
                    "placement": placement.clone(),
                },
                "required": ["host_id"],
            }),
        ),
        tool(
            "read_pane",
            "Read a pane's visible text or scrollback.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "full_scrollback": {
                        "type": "boolean",
                        "description": "Defaults to false; may be long.",
                    },
                },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "send_input",
            "Send text, then keys, to a running program.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "text": input["text"],
                    "keys": input["keys"],
                },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "new_pane",
            "Open a shell or command pane.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Omit for a shell." },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "cwd": { "type": "string", "description": "Defaults to the focused pane's." },
                    "name": { "type": "string" },
                    "floating": { "type": "boolean" },
                    "in_new_tab": { "type": "boolean" },
                },
            }),
        ),
        tool(
            "new_remote_pane",
            "Open a shell or command pane whose process runs on another machine over ssh. \
             Needs a zellij of the same version on the remote machine's PATH.",
            json!({
                "type": "object",
                "properties": {
                    "remote_host": {
                        "type": "string",
                        "description": "An ssh destination: a host, an alias from ssh config, or user@host.",
                    },
                    "command": {
                        "type": "string",
                        "description": "Omit for that machine's login shell.",
                    },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "cwd": {
                        "type": "string",
                        "description": "A path on the remote machine. Defaults to the remote home directory.",
                    },
                    "name": { "type": "string" },
                    "floating": { "type": "boolean" },
                    "in_new_tab": { "type": "boolean" },
                },
                "required": ["remote_host"],
            }),
        ),
        tool(
            "new_tab",
            "Open a new tab.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "cwd": { "type": "string", "description": "Defaults to the focused pane's." },
                },
            }),
        ),
        tool(
            "focus",
            "Focus a pane or tab; give one target.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "tab_id": { "type": "integer" },
                },
            }),
        ),
        tool(
            "rename",
            "Rename a pane or tab; give one target.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "tab_id": { "type": "integer" },
                    "name": { "type": "string" },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "move_pane",
            "Move one tiled pane toward a unique neighbor; use swap_panes for two named panes, set_floating_pane_geometry for floating panes.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "direction": {
                        "type": "string",
                        "enum": ["left", "right", "up", "down"],
                        "description": "Omit for the next layout position.",
                    },
                },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "swap_panes",
            "Swap two adjacent tiled panes.",
            json!({
                "type": "object",
                "properties": {
                    "panes": {
                        "type": "array",
                        "items": pane_ref,
                        "minItems": 2,
                        "maxItems": 2,
                    },
                },
                "required": ["panes"],
            }),
        ),
        tool(
            "resize_pane",
            "Grow or shrink a pane.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "resize": { "type": "string", "enum": ["increase", "decrease"] },
                    "direction": {
                        "type": "string",
                        "enum": ["left", "right", "up", "down"],
                        "description": "Omit for all sides.",
                    },
                },
                "required": ["pane_id", "is_plugin", "resize"],
            }),
        ),
        tool(
            "toggle_pane",
            "Toggle fullscreen or floating state.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "what": { "type": "string", "enum": ["fullscreen", "floating"] },
                },
                "required": ["pane_id", "is_plugin", "what"],
            }),
        ),
        tool(
            "move_panes_to_tab",
            "Move panes to an existing or new tab; give one target.",
            json!({
                "type": "object",
                "properties": {
                    "panes": { "type": "array", "items": pane_ref, "minItems": 1 },
                    "tab_id": { "type": "integer" },
                    "new_tab": { "type": "boolean" },
                    "new_tab_name": { "type": "string" },
                },
                "required": ["panes"],
            }),
        ),
        tool(
            "stack_panes",
            "Stack panes in the current layout.",
            json!({
                "type": "object",
                "properties": {
                    "panes": { "type": "array", "items": pane_ref, "minItems": 2 },
                },
                "required": ["panes"],
            }),
        ),
        tool(
            "set_pane_pinned",
            "Pin or unpin a floating pane.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "pinned": { "type": "boolean" },
                },
                "required": ["pane_id", "is_plugin", "pinned"],
            }),
        ),
        tool(
            "set_floating_pane_geometry",
            "Set floating-pane position or size in cells or percentages.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "x": { "type": "string" },
                    "y": { "type": "string" },
                    "width": { "type": "string" },
                    "height": { "type": "string" },
                },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "rerun_pane",
            "Re-run a command pane.",
            json!({
                "type": "object",
                "properties": { "pane_id": pane["pane_id"], "is_plugin": pane["is_plugin"] },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "scroll_pane",
            "Scroll a pane.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "scroll": { "type": "string", "enum": ["line_up", "line_down", "page_up", "page_down", "top", "bottom"] },
                },
                "required": ["pane_id", "is_plugin", "scroll"],
            }),
        ),
        tool(
            "clear_pane",
            "Clear pane scrollback.",
            json!({
                "type": "object",
                "properties": { "pane_id": pane["pane_id"], "is_plugin": pane["is_plugin"] },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "close_pane",
            "Close a pane and its process.",
            json!({
                "type": "object",
                "properties": { "pane_id": pane["pane_id"], "is_plugin": pane["is_plugin"] },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "close_tab",
            "Close a tab and its panes.",
            json!({
                "type": "object",
                "properties": {
                    "tab_id": { "type": "integer" },
                },
            }),
        ),
        tool(
            "web_search",
            "Search the web for current information; returns snippets, not pages.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                },
                "required": ["query"],
            }),
        ),
        tool(
            "linear_search",
            "Full-text search Linear issues; returns one line each, not full issues.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer" },
                },
                "required": ["query"],
            }),
        ),
        tool(
            "linear_issues",
            "List Linear issues; defaults to your own open ones. assignee is \"me\" or a display name, team a key like ENG, updated_since an ISO date.",
            json!({
                "type": "object",
                "properties": {
                    "assignee": { "type": "string" },
                    "team": { "type": "string" },
                    // Closed rather than a free string: these are Linear's
                    // workflow *types*, and a model left to itself sends a
                    // state name like "In Progress", which matches nothing and
                    // says nothing about why. "open" is ours: not done, not
                    // cancelled.
                    "state": {
                        "type": "string",
                        "enum": ["triage", "backlog", "unstarted", "started", "completed", "canceled", "open"],
                    },
                    "project": { "type": "string" },
                    "label": { "type": "string" },
                    "updated_since": { "type": "string" },
                    "limit": { "type": "integer" },
                },
            }),
        ),
        tool(
            "linear_issue",
            "Read one Linear issue in full by identifier (ENG-123) or UUID: description, state, labels and comments.",
            json!({
                "type": "object",
                "properties": {
                    "identifier": { "type": "string" },
                    "comments": { "type": "boolean" },
                },
                "required": ["identifier"],
            }),
        ),
        tool(
            "schedule",
            "Repeat a prompt until cancelled; unschedule it when done. Runs decline gated tools \
         unless auto_confirm.",
            json!({
                "type": "object",
                "properties": {
                    "seconds": { "type": "number" },
                    "prompt": { "type": "string" },
                    "show": { "type": "boolean", "description": "Show Hocus when the job runs; defaults false." },
                    "auto_confirm": { "type": "boolean", "description": "Skip later confirmations after one initial confirmation; defaults false." },
                },
                "required": ["seconds", "prompt"],
            }),
        ),
        tool(
            "unschedule",
            "Cancel one job, or all if job_id is omitted.",
            json!({
                "type": "object",
                "properties": {
                    "job_id": { "type": "integer" },
                },
            }),
        ),
        tool(
            "project_list",
            "List projects, roots, worktrees, tabs, and the active project.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "project_assign",
            "Assign a tab to a project; new projects require root.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "root": { "type": "string", "description": "Absolute; required when new." },
                    "tab_id": { "type": "integer", "description": "Defaults to the active tab." },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "project_forget",
            "Forget a project; todos remain and discovered projects reappear while an agent runs.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "project_open",
            "Open a project in a new assigned tab.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "root": { "type": "string", "description": "Absolute; required when new." },
                    "tab_name": { "type": "string", "description": "Defaults to the project name." },
                },
                "required": ["name"],
            }),
        ),
        // ponytail: `parse_command` splits key=value pairs on whitespace, so a
        // typed `/todo_add text=fix the bug` cannot work. The form that does is
        // `/todo_add {"text": "fix the bug"}`.
        tool(
            "todo_add",
            "Add a persistent Markdown todo, optionally under a parent.",
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "project": { "type": "string", "description": "Defaults to the active project." },
                    "parent_id": { "type": "integer" },
                },
                "required": ["text"],
            }),
        ),
        tool(
            "todo_list",
            "List a project's todo tree and item IDs.",
            json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Defaults to the active project." },
                    "include_done": { "type": "boolean", "description": "Defaults to false." },
                },
            }),
        ),
        tool(
            "todo_search",
            "Search todo text, including completed items.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "project": { "type": "string", "description": "Defaults to the active project." },
                    "all_projects": { "type": "boolean", "description": "Overrides project; defaults false." },
                },
                "required": ["query"],
            }),
        ),
        tool(
            "todo_done",
            "Complete a todo subtree, or reopen it with undo.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "project": { "type": "string", "description": "Defaults to the active project." },
                    "undo": { "type": "boolean", "description": "Defaults to false." },
                },
                "required": ["id"],
            }),
        ),
        tool(
            "todo_edit",
            "Change a todo item's Markdown.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "text": { "type": "string" },
                    "project": { "type": "string", "description": "Defaults to the active project." },
                },
                "required": ["id", "text"],
            }),
        ),
        tool(
            "agent_register",
            "Register an undetected terminal pane as an agent.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "kind": {
                        "type": "string",
                        "description": "Lowercase letters, digits, _ or -; max 32.",
                    },
                },
                "required": ["pane_id", "kind"],
            }),
        ),
        tool(
            "agent_list",
            "List agent panes, optionally by project.",
            json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                },
            }),
        ),
        tool(
            "agent_send",
            "Send text or keys to a listed agent; prompts usually need enter.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "text": input["text"],
                    "keys": input["keys"],
                },
                "required": ["pane_id"],
            }),
        ),
        tool(
            "agent_control",
            "Interrupt, restart, or close a listed agent.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "action": {
                        "type": "string",
                        "enum": ["interrupt", "restart", "close"],
                    },
                },
                "required": ["pane_id", "action"],
            }),
        ),
        tool(
            "agent_spawn",
            "Start a coding agent in a new pane; `branch` cuts a git worktree for it first. Not \
             for writing Zellij Lua plugins - use plugin_write.",
            json!({
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "enum": launch::OFFERED,
                        "description": "The agent to run. Either this or command.",
                    },
                    "command": { "type": "string", "description": "A program to run instead." },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "prompt": { "type": "string", "description": "What to start the agent on." },
                    "model": { "type": "string" },
                    "cwd": { "type": "string", "description": "Defaults to the focused pane's." },
                    "project": { "type": "string", "description": "Defaults to the active project." },
                    "kind": {
                        "type": "string",
                        "description": "For unrecognized agent commands.",
                    },
                    "branch": {
                        "type": "string",
                        "description": "Branch to create; omit to work in the current checkout.",
                    },
                    "base": {
                        "type": "string",
                        "description": format!("Branched from. Defaults to {}.", launch::DEFAULT_BASE),
                    },
                    "worktree": {
                        "type": "boolean",
                        "description": "Defaults true when branch is given.",
                    },
                    "worktree_path": {
                        "type": "string",
                        "description": format!("From the repo root. Defaults to {}<branch>.", launch::WORKTREE_PREFIX),
                    },
                    "env": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Environment for the agent.",
                    },
                    "permission_mode": {
                        "type": "string",
                        "description": format!(
                            "skip = no permission checks; or the agent's own: claude {}; codex {}.",
                            launch::CLAUDE_PERMISSION_MODES[..6].join("/"),
                            launch::CODEX_PERMISSION_MODES[..5].join("/"),
                        ),
                    },
                    "resume": {
                        "type": "string",
                        "description": "Session id to resume, or `last`.",
                    },
                    "hooks": {
                        "type": "boolean",
                        "description": "claude reports its state via hooks; default true.",
                    },
                },
            }),
        ),
        tool(
            "slack_status",
            "Report Slack archive and daemon status; use after empty or failed Slack reads.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "slack_search",
            "Search the local Slack archive with FTS5; use slack_read for context.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                },
                "required": ["query"],
            }),
        ),
        tool(
            "slack_read",
            "Read an archived Slack transcript; backfill then retry if empty.",
            json!({
                "type": "object",
                "properties": slack_properties(),
                "required": ["channel"],
            }),
        ),
        tool(
            "slack_backfill",
            "Fetch a dated Slack channel window into the local archive.",
            json!({
                "type": "object",
                "properties": slack_properties(),
                "required": ["channel"],
            }),
        ),
        tool(
            "slack_post",
            "Post immediately to Slack; thread replies require thread.",
            json!({
                "type": "object",
                "properties": {
                    "channel": slack["channel"],
                    "workspace": slack["workspace"],
                    "text": { "type": "string" },
                    "thread": { "type": "string" },
                },
                "required": ["channel", "text"],
            }),
        ),
        tool(
            "find_files",
            "Find git-visible files by name inside the active project.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Name fragment or glob." },
                    "path": { "type": "string", "description": "Project-relative subdirectory." },
                    "max_results": { "type": "integer", "description": "Defaults to 300." },
                },
                "required": ["pattern"],
            }),
        ),
        tool(
            "search_files",
            "Regex-search git-visible text files inside the active project; hits carry \
             1-based file line numbers, ready for read_file.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Regular expression." },
                    "path": { "type": "string", "description": "Project-relative subdirectory." },
                    "glob": { "type": "string" },
                    "case_insensitive": { "type": "boolean", "description": "Defaults to false." },
                    "max_results": { "type": "integer", "description": "Defaults to 300." },
                },
                "required": ["query"],
            }),
        ),
        tool(
            "read_file",
            "Read a line range from a file inside the active project.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Project-relative or returned absolute path." },
                    "start_line": { "type": "integer", "description": "A file line number, 1-based as every editor counts them; defaults to 1. Unrelated to any ID." },
                    "line_count": { "type": "integer", "description": "Defaults to 300." },
                },
                "required": ["path"],
            }),
        ),
        tool(
            "git_log",
            "Active project history.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "before_commit": { "type": "string" },
                    "max_results": { "type": "integer" },
                },
            }),
        ),
        tool(
            "git_show",
            "Active project git_log commit.",
            json!({
                "type": "object",
                "properties": {
                    "commit": { "type": "string" },
                    "path": { "type": "string" },
                    "patch": { "type": "boolean" },
                },
                "required": ["commit"],
            }),
        ),
        tool(
            "github_read",
            "Active project GitHub; GET only.",
            json!({
                "type": "object",
                "properties": {
                    "endpoint": { "type": "string" },
                },
            }),
        ),
        tool(
            "skill_list",
            "List user and active-project skills, including disabled, shadowed and refused entries.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "skill_read",
            "Read a complete SKILL.md. Without `scope`, reads the effective enabled skill.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "scope": { "type": "string", "enum": ["user", "project"] },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "skill_write",
            "Author a skill yourself, never by spawning an agent. `source` is the complete \
             SKILL.md, not a patch; user scope is the default, and `overwrite` is required to \
             replace one that exists. Use $ARGUMENTS where the typed command's tail belongs.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "source": { "type": "string" },
                    "scope": { "type": "string", "enum": ["user", "project"], "description": "Defaults to user." },
                    "overwrite": { "type": "boolean" },
                },
                "required": ["name", "source"],
            }),
        ),
        tool(
            "skill_enable",
            "Enable one user or project skill. A newly enabled project skill shadows its user copy.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "scope": { "type": "string", "enum": ["user", "project"] },
                },
                "required": ["name", "scope"],
            }),
        ),
        tool(
            "skill_disable",
            "Disable one user or project skill without deleting it. Disabling a project copy reveals an enabled user copy.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "scope": { "type": "string", "enum": ["user", "project"] },
                },
                "required": ["name", "scope"],
            }),
        ),
        // Property descriptions are folded into the tool descriptions throughout: guidance
        // costs one copy per tool there, against one copy per property here. `name` is a bare
        // file name with an optional `.lua` for all three tools that take one.
        tool(
            "plugin_docs",
            "How to write a Zellij Lua plugin. Call before writing one. `section` defaults to \
             index, which lists the rest.",
            json!({
                "type": "object",
                "properties": {
                    "section": { "type": "string", "enum": lua_docs::SECTIONS },
                },
            }),
        ),
        tool(
            "plugin_list",
            "List the user's Lua plugins.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "plugin_read",
            "Read one of the user's Lua plugins. `name` is a file name, .lua optional.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "start_line": { "type": "integer" },
                    "line_count": { "type": "integer" },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "plugin_check",
            "Compile a Lua plugin source without writing or loading it, answering with the \
             compile error or that it compiles. plugin_write runs this check itself and refuses \
             a source that fails it, so call this to try a fix before writing, not after. A \
             source that compiles can still fail at load; plugin_reload shows that.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "source": { "type": "string" },
                },
                "required": ["name", "source"],
            }),
        ),
        tool(
            "plugin_write",
            "Write a Lua plugin. This is how a plugin is authored: you write it yourself, never \
             by spawning an agent to do it. Call plugin_docs first. `source` is the complete \
             file, not a patch; it is compiled before anything is asked or written, and a \
             compile error is the answer. Approving the write grants the permissions the source \
             requests, and plugin_reload comes next. `overwrite` is required to replace one \
             that exists. `name` may be `folder/module` for a file the plugin loads with \
             require(\"folder.module\").",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "source": { "type": "string" },
                    "overwrite": { "type": "boolean" },
                },
                "required": ["name", "source"],
            }),
        ),
        tool(
            "plugin_reload",
            "Load or reload one of the user's Lua plugins, opening it in a pane if it is not \
             running. Answers with the pane's id and what it shows once loaded - the plugin's \
             first render, or the load error. plugin_log has anything it logged.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "plugin_log",
            "The last lines one of the user's Lua plugins wrote with zellij.log or print, from \
             zellij's log. `line_count` defaults to 50.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "line_count": { "type": "integer" },
                },
                "required": ["name"],
            }),
        ),
    ])
}

/// The tools planning mode leaves advertised: everything that only observes.
///
/// An explicit allowlist rather than the complement of [`needs_confirmation`],
/// which answers a different and much weaker question — "destructive enough to
/// stop for a y/n". `new_pane`, `focus`, `close_pane` and `todo_add` all change
/// something without asking, so deriving one list from the other would let every
/// tool added in future widen planning mode by default. Here a tool is read-only
/// only because someone wrote it down.
///
/// The near misses, since they are the ones that will be argued about:
/// `slack_backfill` reads Slack but writes the local archive; `schedule` arms a
/// recurring billed run, which a plan should propose rather than start;
/// `project_open`, `project_assign` and `agent_register` change state that
/// outlives the turn; and `focus`, `rename` and `scroll_pane` are harmless to
/// the session but move the screen under someone who is reading a plan.
pub const READ_ONLY: &[&str] = &[
    // The session, as it is.
    "list_session",
    "list_other_sessions",
    "list_orphaned_panes",
    "read_pane",
    "agent_list",
    // The active project's code and history.
    "project_list",
    "find_files",
    "search_files",
    "read_file",
    "git_log",
    "git_show",
    // GET-only by construction; see its own schema.
    "github_read",
    // The world outside.
    "web_search",
    "linear_search",
    "linear_issues",
    "linear_issue",
    "slack_status",
    "slack_search",
    "slack_read",
    // Lists and plugin sources, read but not written.
    "todo_list",
    "todo_search",
    "plugin_docs",
    "plugin_check",
    "plugin_list",
    "plugin_read",
    "plugin_log",
    "skill_list",
    "skill_read",
];

/// The `tools` array as planning mode sends it: [`specs`] minus everything that
/// acts.
///
/// Filtered out of the real thing rather than built from a second list, so a
/// tool cannot become readable by being forgotten — it has to be named in
/// [`READ_ONLY`] to survive this.
pub fn read_only_specs() -> Value {
    let Value::Array(specs) = specs() else {
        return json!([]);
    };
    Value::Array(
        specs
            .into_iter()
            .filter(|spec| {
                spec["function"]["name"]
                    .as_str()
                    .is_some_and(|name| READ_ONLY.contains(&name))
            })
            .collect(),
    )
}

fn tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": { "name": name, "description": description, "parameters": parameters },
    })
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneRef {
    pub pane_id: u32,
    pub is_plugin: bool,
}

impl PaneRef {
    fn id(&self) -> PaneId {
        if self.is_plugin {
            PaneId::Plugin(self.pane_id)
        } else {
            PaneId::Terminal(self.pane_id)
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum MoveDestination {
    ExistingId(usize),
    New(Option<String>),
}

#[derive(Debug, PartialEq)]
enum ScrollAction {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

impl PaneDirection {
    fn parse(name: Option<&str>) -> Result<Option<Self>, String> {
        match name {
            None => Ok(None),
            Some("left") => Ok(Some(Self::Left)),
            Some("right") => Ok(Some(Self::Right)),
            Some("up") => Ok(Some(Self::Up)),
            Some("down") => Ok(Some(Self::Down)),
            Some(_) => Err("direction must be left, right, up or down".to_owned()),
        }
    }
}

#[derive(Debug, Clone)]
struct PanePosition {
    id: PaneId,
    /// An option rather than a sentinel: panes with no tab would otherwise compare as
    /// "the same tab" in `plan_swap`.
    tab_position: Option<usize>,
    x: usize,
    y: usize,
    columns: usize,
    rows: usize,
    visible: bool,
    selectable: bool,
    floating: bool,
    fullscreen: bool,
    floating_layer_visible: bool,
}

impl PanePosition {
    fn can_move(&self) -> bool {
        self.visible
            && self.selectable
            && !self.floating
            && !self.fullscreen
            && !self.floating_layer_visible
    }
}

fn plan_swap(
    panes: &[PanePosition],
    first_id: PaneId,
    second_id: PaneId,
) -> Result<PaneDirection, String> {
    if first_id == second_id {
        return Err("swap_panes needs two different panes".to_owned());
    }
    let first = panes
        .iter()
        .find(|pane| pane.id == first_id)
        .ok_or_else(|| "pane is not in the current session; call list_session again".to_owned())?;
    let second = panes
        .iter()
        .find(|pane| pane.id == second_id)
        .ok_or_else(|| "pane is not in the current session; call list_session again".to_owned())?;
    if first.tab_position != second.tab_position {
        return Err("swap_panes needs both panes in the same tab".to_owned());
    }
    if !first.can_move() || !second.can_move() {
        return Err("swap_panes only supports visible selectable tiled panes".to_owned());
    }
    let direction = adjacent_direction(first, second)
        .ok_or_else(|| "panes are not adjacent; do not retry with move_pane".to_owned())?;
    let neighbors = panes
        .iter()
        .filter(|pane| {
            pane.id != first.id && pane.tab_position == first.tab_position && pane.can_move()
        })
        .filter(|pane| adjacent_direction(first, pane) == Some(direction))
        .count();
    if neighbors != 1 {
        return Err("the requested edge has multiple adjacent panes; swap is ambiguous".to_owned());
    }
    let opposite = adjacent_direction(second, first).expect("adjacency is symmetric");
    let reverse_neighbors = panes
        .iter()
        .filter(|pane| {
            pane.id != second.id && pane.tab_position == second.tab_position && pane.can_move()
        })
        .filter(|pane| adjacent_direction(second, pane) == Some(opposite))
        .count();
    if reverse_neighbors != 1 {
        return Err("the requested edge has multiple adjacent panes; swap is ambiguous".to_owned());
    }
    Ok(direction)
}

fn plan_move(
    panes: &[PanePosition],
    source_id: PaneId,
    direction: PaneDirection,
) -> Result<PaneId, String> {
    let source = panes
        .iter()
        .find(|pane| pane.id == source_id)
        .ok_or_else(|| "pane is not in the current session; call list_session again".to_owned())?;
    if !source.can_move() {
        return Err(
            "move_pane only supports visible selectable tiled panes; use set_floating_pane_geometry for floating panes"
                .to_owned(),
        );
    }
    let mut neighbors = panes
        .iter()
        .filter(|pane| {
            pane.id != source.id && pane.tab_position == source.tab_position && pane.can_move()
        })
        .filter(|pane| adjacent_direction(source, pane) == Some(direction));
    let neighbor = neighbors
        .next()
        .ok_or_else(|| "there is no adjacent pane in that direction".to_owned())?;
    if neighbors.next().is_some() {
        return Err(
            "the requested direction has multiple adjacent panes; move is ambiguous".to_owned(),
        );
    }
    Ok(neighbor.id)
}

fn adjacent_direction(first: &PanePosition, second: &PanePosition) -> Option<PaneDirection> {
    let horizontal_overlap = first.y < second.y.saturating_add(second.rows)
        && second.y < first.y.saturating_add(first.rows);
    let vertical_overlap = first.x < second.x.saturating_add(second.columns)
        && second.x < first.x.saturating_add(first.columns);
    if second.x.saturating_add(second.columns) == first.x && horizontal_overlap {
        Some(PaneDirection::Left)
    } else if first.x.saturating_add(first.columns) == second.x && horizontal_overlap {
        Some(PaneDirection::Right)
    } else if second.y.saturating_add(second.rows) == first.y && vertical_overlap {
        Some(PaneDirection::Up)
    } else if first.y.saturating_add(first.rows) == second.y && vertical_overlap {
        Some(PaneDirection::Down)
    } else {
        None
    }
}

/// Every field any tool takes. One struct rather than one per tool: serde ignores the
/// ones a given tool doesn't use, and dispatch checks what it needs.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Args {
    pub pane_id: Option<u32>,
    pub is_plugin: Option<bool>,
    pub tab_id: Option<usize>,
    pub new_tab: bool,
    pub new_tab_name: Option<String>,
    pub panes: Vec<PaneRef>,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub remote_host: Option<String>,
    pub command: Option<Value>,
    pub args: Vec<String>,
    pub floating: bool,
    pub in_new_tab: bool,
    pub full_scrollback: bool,
    pub text: Option<String>,
    pub keys: Vec<String>,
    pub resize: Option<String>,
    pub direction: Option<String>,
    pub what: Option<String>,
    pub pinned: Option<bool>,
    pub x: Option<String>,
    pub y: Option<String>,
    pub width: Option<String>,
    pub height: Option<String>,
    pub scroll: Option<String>,
    pub seconds: Option<f64>,
    pub prompt: Option<String>,
    pub query: Option<String>,
    pub job_id: Option<u64>,
    pub show: bool,
    pub auto_confirm: bool,
    pub kind: Option<String>,
    // Starting an agent. `command`, `args`, `cwd`, `project`, `kind` and `prompt`
    // above are `agent_spawn`'s too — `prompt` is `schedule`'s, reused rather than
    // shadowed. `worktree_path` is spelled out because `path` is the file tools'.
    pub agent: Option<String>,
    pub model: Option<String>,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub worktree: Option<bool>,
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub permission_mode: Option<String>,
    pub resume: Option<String>,
    pub hooks: Option<bool>,
    // The todo tools. `text`, `query` and `project` above are theirs as well.
    pub id: Option<u64>,
    pub parent_id: Option<u64>,
    pub include_done: bool,
    pub all_projects: bool,
    pub undo: bool,
    pub project: Option<String>,
    pub root: Option<String>,
    pub tab_name: Option<String>,
    pub action: Option<String>,
    pub channel: Option<String>,
    pub workspace: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub thread: Option<String>,
    // Moving panes between sessions. `to_session` rather than `to`, which the
    // Slack tools already own.
    pub to_session: Option<String>,
    pub from_session: Option<String>,
    pub placement: Option<String>,
    pub host_id: Option<String>,
    // The file tools. `query` above is `search_files`'s pattern.
    pub path: Option<String>,
    pub pattern: Option<String>,
    pub glob: Option<String>,
    pub case_insensitive: bool,
    pub max_results: Option<usize>,
    pub start_line: Option<usize>,
    pub line_count: Option<usize>,
    // Read-only repository tools.
    pub before_commit: Option<String>,
    pub commit: Option<String>,
    pub patch: bool,
    pub endpoint: Option<String>,
    // The Linear tools. `query` above is `linear_search`'s term and `project`
    // the todo tools', reused here rather than shadowed.
    pub identifier: Option<String>,
    pub assignee: Option<String>,
    pub team: Option<String>,
    pub state: Option<String>,
    pub label: Option<String>,
    pub updated_since: Option<String>,
    pub limit: Option<usize>,
    /// `Option` rather than a bare `bool` because it defaults *true* — the
    /// plain flags in this struct all default false.
    pub comments: Option<bool>,
    // The plugin-authoring tools. `name` above is theirs as well, as are
    // `start_line` and `line_count`, which are `read_file`'s.
    pub source: Option<String>,
    pub overwrite: bool,
    pub section: Option<String>,
    // Skill tier. Separate from `source`, which is the complete file text.
    pub scope: Option<String>,
}

impl Args {
    /// An absent argument object is `{}`, which several providers send as an
    /// empty string rather than the literal.
    pub fn parse(arguments: &str) -> Result<Self, String> {
        let arguments = arguments.trim();
        if arguments.is_empty() {
            return Ok(Args::default());
        }
        serde_json::from_str(arguments).map_err(|error| format!("bad arguments: {error}"))
    }

    /// Where a warped pane should land. Absent means a tab of its own: the only
    /// placement that cannot disturb a layout the receiving session has, and so
    /// the only safe thing to assume when the model did not say.
    pub fn placement(&self) -> Result<WarpPlacement, String> {
        match self.placement.as_deref() {
            None | Some("new_tab") => Ok(WarpPlacement::NewTab),
            Some("focused_tab") => Ok(WarpPlacement::FocusedTab),
            Some("floating") => Ok(WarpPlacement::Floating),
            Some(other) => Err(format!(
                "unknown placement {other:?}; use new_tab, focused_tab or floating"
            )),
        }
    }

    /// `command` is a string with a separate `args` array for `new_pane`, but
    /// models often send the whole argv as one array. Accept either.
    pub fn command_words(&self) -> Vec<String> {
        match &self.command {
            Some(Value::String(word)) => {
                let mut words = vec![word.clone()];
                words.extend(self.args.iter().cloned());
                words
            },
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The program `new_pane`/`new_remote_pane` would run, with its args: the
    /// first non-empty word and everything after it.
    ///
    /// `command:""` is a command in form only, and models also send a leading
    /// empty word (`{"command":"","args":["htop"]}`); both are skipped here so
    /// the confirmation prompt, its wording and the dispatcher all read the same
    /// program — the prompt must never say `htop` and then open a bare shell, or
    /// the reverse. Empty words *after* the program are kept; they may be
    /// deliberate arguments.
    pub fn program_and_args(&self) -> Option<(String, Vec<String>)> {
        let mut words = self
            .command_words()
            .into_iter()
            .skip_while(String::is_empty);
        let program = words.next()?;
        Some((program, words.collect()))
    }

    /// What starting this agent means, in the terms `hocus-new-agent` says it in.
    ///
    /// A reading, no more: every default and every rule about what pairs with what
    /// is `launch`'s, so the modal's Create button and this tool cannot come to
    /// disagree about what "a claude on a worktree off main" is.
    pub fn launch_spec(&self) -> launch::Spec {
        // `command_words` folds `args` in behind the program name, which is the shape
        // `new_pane` wants and not `launch`'s — and it answers with nothing at all
        // when there is no `command`, which for an `agent` call is every time. So the
        // program comes off the front only when there was one to come off.
        let mut words = self.command_words().into_iter();
        let (command, args) = match words.next() {
            Some(program) => (Some(program), words.collect()),
            None => (None, self.args.clone()),
        };
        launch::Spec {
            kind: self.agent.clone(),
            command,
            args,
            model: self.model.clone(),
            prompt: self.prompt.clone(),
            branch: self.branch.clone(),
            base: self.base.clone(),
            worktree: self.worktree,
            worktree_path: self.worktree_path.clone(),
            env: self.env.clone(),
            permission_mode: self.permission_mode.clone(),
            resume: self.resume.clone(),
            hooks: self.hooks,
        }
    }

    /// One write per key, and the text as its own bracketed paste, rather than
    /// a single `text + key` buffer. Both ends match on whole buffers: a TUI
    /// sees `"hello\r"` as one chunk that is neither a keypress nor a clean
    /// paste, so the Enter never submits; and Zellij's own key adjustment
    /// (LNM `\r` -> `\r\n`, cursor-key-mode arrows) only fires for a buffer
    /// that is exactly one key. The paste-end marker is what makes the CR that
    /// follows unambiguously a keypress, with no dependence on read timing.
    ///
    /// `bracket_paste` is for terminal panes only: Zellij drops a marker that
    /// is a buffer of its own when the pane is not in bracketed paste mode.
    /// A plugin pane treats the markers differently again — it swallows
    /// everything between them and delivers it as one `Event::PastedText`,
    /// so a plugin never sees the keys the markers were meant to disambiguate.
    pub fn input_chunks(&self, bracket_paste: bool) -> Result<Vec<Vec<u8>>, String> {
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        let text = self.text.clone().unwrap_or_default().into_bytes();
        if !text.is_empty() {
            if bracket_paste {
                chunks.push(b"\x1b[200~".to_vec());
                chunks.push(text);
                chunks.push(b"\x1b[201~".to_vec());
            } else {
                chunks.push(text);
            }
        }
        for key in &self.keys {
            let bytes: &[u8] = match key.as_str() {
                "enter" => b"\r",
                "tab" => b"\t",
                "escape" => b"\x1b",
                "backspace" => b"\x7f",
                "delete" => b"\x1b[3~",
                "insert" => b"\x1b[2~",
                "up" => b"\x1b[A",
                "down" => b"\x1b[B",
                "left" => b"\x1b[D",
                "right" => b"\x1b[C",
                "home" => b"\x1b[H",
                "end" => b"\x1b[F",
                "page_up" => b"\x1b[5~",
                "page_down" => b"\x1b[6~",
                "ctrl_a" => b"\x01",
                "ctrl_b" => b"\x02",
                "ctrl_c" => b"\x03",
                "ctrl_d" => b"\x04",
                "ctrl_e" => b"\x05",
                "ctrl_f" => b"\x06",
                "ctrl_k" => b"\x0b",
                "ctrl_l" => b"\x0c",
                "ctrl_u" => b"\x15",
                "ctrl_w" => b"\x17",
                "ctrl_y" => b"\x19",
                "ctrl_z" => b"\x1a",
                other => return Err(format!("unsupported key: {other}")),
            };
            chunks.push(bytes.to_vec());
        }
        if chunks.is_empty() {
            Err("send_input needs non-empty text or at least one key".to_owned())
        } else {
            Ok(chunks)
        }
    }

    pub fn pane_ids(&self, minimum: usize) -> Result<Vec<PaneId>, String> {
        if self.panes.len() < minimum {
            return Err(format!("at least {minimum} panes are required"));
        }
        Ok(self.panes.iter().map(PaneRef::id).collect())
    }

    pub fn swap_pane_ids(&self) -> Result<[PaneId; 2], String> {
        match self.panes.as_slice() {
            [first, second] => Ok([first.id(), second.id()]),
            _ => Err("swap_panes needs exactly two panes".to_owned()),
        }
    }

    pub fn move_destination(&self) -> Result<MoveDestination, String> {
        match (self.tab_id, self.new_tab) {
            (Some(id), false) if self.new_tab_name.is_none() => {
                Ok(MoveDestination::ExistingId(id))
            },
            (None, true) => Ok(MoveDestination::New(self.new_tab_name.clone())),
            _ => Err("set exactly one of tab_id or new_tab=true; new_tab_name is only valid for a new tab".to_owned()),
        }
    }

    pub fn tab_target(&self) -> Result<usize, String> {
        self.tab_id
            .ok_or_else(|| "tab_id is required; call list_session to get them".to_owned())
    }

    /// `focus` and `rename` take either a tab or a pane pair. `None` means the
    /// call named no tab at all, so the caller falls through to the pane path.
    pub fn tab_or_pane_target(&self) -> Option<Result<usize, String>> {
        let tab_id = self.tab_id?;
        Some(if self.pane_id.is_some() || self.is_plugin.is_some() {
            Err("give exactly one of pane pair or tab_id".to_owned())
        } else {
            Ok(tab_id)
        })
    }

    pub fn floating_geometry(&self) -> Result<FloatingPaneCoordinates, String> {
        fn validate(name: &str, value: &Option<String>, allow_zero: bool) -> Result<(), String> {
            let Some(value) = value else { return Ok(()) };
            // The parsed value is thrown away: `FloatingPaneCoordinates::new` takes the
            // strings. This is only here to answer a bad one in words the model can act on.
            percent_or_fixed(name, value, allow_zero).map(|_| ())
        }

        validate("x", &self.x, true)?;
        validate("y", &self.y, true)?;
        validate("width", &self.width, false)?;
        validate("height", &self.height, false)?;
        FloatingPaneCoordinates::new(
            self.x.clone(),
            self.y.clone(),
            self.width.clone(),
            self.height.clone(),
            None,
            None,
        )
        .ok_or_else(|| "at least one of x, y, width or height is required".to_owned())
    }

    pub fn terminal_pane_id(&self) -> Result<u32, String> {
        match (self.pane_id, self.is_plugin) {
            (Some(id), Some(false)) => Ok(id),
            (_, Some(true)) => Err("rerun_pane only supports terminal command panes".to_owned()),
            _ => Err(
                "pane_id and is_plugin are both required; call list_session to get them".to_owned(),
            ),
        }
    }

    fn scroll_action(&self) -> Result<ScrollAction, String> {
        match self.scroll.as_deref() {
            Some("line_up") => Ok(ScrollAction::LineUp),
            Some("line_down") => Ok(ScrollAction::LineDown),
            Some("page_up") => Ok(ScrollAction::PageUp),
            Some("page_down") => Ok(ScrollAction::PageDown),
            Some("top") => Ok(ScrollAction::Top),
            Some("bottom") => Ok(ScrollAction::Bottom),
            _ => Err(
                "scroll must be line_up, line_down, page_up, page_down, top or bottom".to_owned(),
            ),
        }
    }
}

/// The Linear tools. Intercepted in `run_head` rather than dispatched here, for
/// the same two reasons `web_search` is: the API key comes from the resolved
/// config, and the answer arrives later as a `WebRequestResult`.
pub const LINEAR_TOOLS: [&str; 3] = ["linear_search", "linear_issues", "linear_issue"];

/// The tools backed by the `dobbs` CLI. Intercepted in `run_head` rather than
/// dispatched here, because the binary to run comes from the resolved config.
pub const SLACK_TOOLS: [&str; 5] = [
    "slack_status",
    "slack_search",
    "slack_read",
    "slack_backfill",
    "slack_post",
];

/// The `dobbs` arguments a Slack call becomes, without the binary itself — the
/// caller prepends that, since only it has the config. Pure so the mapping is
/// testable off-host; nothing here talks to dobbs or to Zellij.
pub fn slack_argv(name: &str, args: &Args) -> Result<Vec<String>, String> {
    fn required<'a>(value: &'a Option<String>, field: &str) -> Result<&'a str, String> {
        match value.as_deref().map(str::trim) {
            Some(value) if !value.is_empty() => Ok(value),
            _ => Err(format!("{field} is required and must not be empty")),
        }
    }

    // Optional flags are omitted entirely when absent: dobbs reads an empty
    // -from as an open bound, but an empty -channel as an error.
    fn flag(argv: &mut Vec<String>, name: &str, value: &Option<String>) {
        if let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            argv.push(format!("-{name}"));
            argv.push(value.to_owned());
        }
    }

    let mut argv = Vec::new();
    match name {
        "slack_status" => Ok(vec!["status".to_owned()]),
        // A query is positional and joined by dobbs itself, so FTS5 operators
        // survive as the model wrote them.
        "slack_search" => Ok(vec![
            "search".to_owned(),
            required(&args.query, "query")?.to_owned(),
        ]),
        "slack_read" | "slack_backfill" => {
            required(&args.channel, "channel")?;
            argv.push(match name {
                "slack_read" => "export-channel".to_owned(),
                _ => "backfill-channel".to_owned(),
            });
            flag(&mut argv, "channel", &args.channel);
            flag(&mut argv, "workspace", &args.workspace);
            flag(&mut argv, "from", &args.from);
            flag(&mut argv, "to", &args.to);
            Ok(argv)
        },
        "slack_post" => {
            required(&args.channel, "channel")?;
            required(&args.text, "text")?;
            argv.push("post".to_owned());
            flag(&mut argv, "channel", &args.channel);
            flag(&mut argv, "workspace", &args.workspace);
            flag(&mut argv, "text", &args.text);
            flag(&mut argv, "thread", &args.thread);
            Ok(argv)
        },
        other => Err(format!("no such tool: {other}")),
    }
}

/// The tools that read the project's own files. Intercepted in `run_head` for
/// two reasons: they need the active project, which `dispatch` cannot reach, and
/// the plugin cannot read those files itself.
///
/// A wasm plugin sees only its WASI preopens — `/host`, `/data`, `/cache`,
/// `/tmp` — and a project root is none of those, so `std::fs` cannot open a file
/// in the user's checkout at all. These shell out the way the Slack tools do.
pub const FILE_TOOLS: [&str; 3] = ["find_files", "search_files", "read_file"];
pub const GIT_TOOLS: [&str; 2] = ["git_log", "git_show"];
pub const PROJECT_READ_TOOLS: [&str; 6] = [
    "find_files",
    "search_files",
    "read_file",
    "git_log",
    "git_show",
    "github_read",
];

pub const SKILL_TOOLS: [&str; 5] = [
    "skill_list",
    "skill_read",
    "skill_write",
    "skill_enable",
    "skill_disable",
];

pub const SKILL_FILE_TOOLS: [&str; 4] =
    ["skill_read", "skill_write", "skill_enable", "skill_disable"];

/// The tools that read and write the user's own Zellij plugin folder.
///
/// Intercepted in `run_head` for the same reason the file tools are: they need the plugin
/// folder, which the host exposes as the `/plugins` root (and names through
/// `get_user_plugin_dir`, for the reload URL and the permission grant). Every one answers at
/// once; none runs a command.
pub const PLUGIN_TOOLS: [&str; 7] = [
    "plugin_docs",
    "plugin_check",
    "plugin_list",
    "plugin_read",
    "plugin_write",
    "plugin_reload",
    "plugin_log",
];

/// Compile `source` as the plugin `name` would be loaded, answering the compile error.
///
/// The host does the compiling (`zellij.check_lua_plugin`), with the same interpreter the
/// loader uses, so what passes here parses there. Off wasm there is no host, and every source
/// compiles: the tests exercise what is said around the answer, not the Lua parser.
#[cfg(target_arch = "wasm32")]
pub fn compile_check(name: &str, source: &str) -> Result<(), String> {
    zellij_tile::shim::check_lua_plugin(&chunk_name(name), source)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn compile_check(_name: &str, _source: &str) -> Result<(), String> {
    Ok(())
}

/// The name a compile error reports the plugin under: the file the loader would name, so a
/// `folder/module` reads as `module.lua` here as it does in a load error.
pub fn chunk_name(name: &str) -> String {
    let file = std::path::Path::new(name.trim())
        .file_name()
        .map(|file| file.to_string_lossy().to_string())
        .unwrap_or_else(|| name.trim().to_owned());
    match file.ends_with(".lua") {
        true => file,
        false => format!("{file}.lua"),
    }
}

/// What `plugin_check` answers, and what `plugin_write` says instead of asking to write a
/// plugin that cannot be loaded. The error is the interpreter's, which names the line.
pub fn compile_report(name: &str, source: &str) -> Result<String, String> {
    let chunk = chunk_name(name);
    match compile_check(name, source) {
        Ok(()) => Ok(format!("{chunk} compiles")),
        Err(error) => Err(format!(
            "{chunk} does not compile: {error}\nFix the source and send the whole file again."
        )),
    }
}

/// The refusal for a `plugin_write` whose source will not compile, or `None` when there is
/// nothing to refuse — a write with no source at all is refused later, by `plugin_argv`.
///
/// Checked before the confirmation rather than at write time, so nobody is asked to approve a
/// file the loader would only reject.
pub fn uncompilable_write(call: &ToolCall) -> Option<String> {
    if call.name != "plugin_write" {
        return None;
    }
    let args = Args::parse(&call.arguments).ok()?;
    let name = args.name.as_deref()?;
    let source = args.source.as_deref()?;
    compile_report(name, source).err()
}

/// How many log lines `plugin_log` shows unless asked otherwise.
pub const PLUGIN_LOG_LINES: usize = 50;

/// What `plugin_log` says for the host's answer. An empty answer is the ordinary first one
/// for a plugin that has not logged yet, and the model should be told how to make it say
/// something rather than shown nothing; the host's own error - no log yet, no plugin folder -
/// is passed on as it is.
pub fn plugin_log_answer(name: &str, lines: Result<String, String>) -> String {
    let name = name.trim_end_matches(".lua");
    match lines {
        Ok(lines) if lines.trim().is_empty() => format!(
            "no log lines from {name}.lua yet. A plugin writes one with zellij.log(...) or \
             print(...); a load or runtime error is shown in its pane, not here."
        ),
        Ok(lines) => lines.trim_end_matches('\n').to_owned(),
        Err(error) => error,
    }
}

/// The longest plugin name accepted, so a name cannot be used to build an unreasonable path.
const MAX_PLUGIN_NAME_LEN: usize = 64;

/// Said by every plugin tool when the host could not resolve a folder, so the answer to "where
/// would this go" is the same one whichever tool was asked.
pub const NO_PLUGIN_FOLDER: &str = "no plugin folder is configured; set `user_plugin_dir` in the \
                                    Zellij config, or start a session where the config dir can \
                                    be found";

/// The root the host mounts the user's plugin folder at, for every `fs_*` command.
///
/// The host resolves symlinks and refuses a path that leaves the folder, which is the
/// containment `resolve_plugin_file` can only assert lexically - and it is what retired the
/// shell-side symlink guard these tools used to run in front of every open.
pub const PLUGINS_ROOT: &str = "/plugins";

/// `/plugins/<name>.lua`: the path the host's `fs_*` commands take for a plugin the model named.
/// The same rules as `resolve_plugin_file`, against the root instead of the real folder.
pub fn plugin_fs_path(name: Option<&str>) -> Result<String, String> {
    resolve_plugin_file(PLUGINS_ROOT, name)
}

/// Resolve a plugin the model named against the user's plugin folder.
///
/// Deliberately far stricter than `resolve_in_root`: a plugin folder is nearly flat, so there is
/// no legitimate *path* here, only a file name - or, for a module a plugin `require`s, one
/// folder and a file name (`app/ui`). Refusing everything but those safe components means the
/// containment check at the end can never actually fire — it is kept as the second wall, so
/// that relaxing the character rule later cannot silently become a traversal.
///
/// The name is accepted with or without its `.lua`, and always resolves to the `.lua` file:
/// `plugin_reload` resolves through here too, so the URL it loads and the file that was written
/// cannot disagree.
///
/// Everything here is lexical, which is the whole of what a wasm plugin can check: a symlink in
/// the folder passes every rule below while pointing anywhere at all. The host's `/plugins`
/// root is the other half of the containment - it canonicalizes the path and refuses one that
/// leaves the folder - and every tool that opens the file goes through it.
pub fn resolve_plugin_file(dir: &str, name: Option<&str>) -> Result<String, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err(NO_PLUGIN_FOLDER.to_owned());
    }
    let name = name.map(str::trim).unwrap_or_default();
    if name.is_empty() {
        return Err("name is required and must not be empty".to_owned());
    }
    let name = name.strip_suffix(".lua").unwrap_or(name);
    if name.len() > MAX_PLUGIN_NAME_LEN {
        return Err(format!(
            "a plugin name may be at most {MAX_PLUGIN_NAME_LEN} characters"
        ));
    }
    let components: Vec<&str> = name.split('/').collect();
    if components.len() > 2 {
        return Err(format!(
            "{name:?} is not a plugin name: at most one folder, for a module a plugin requires"
        ));
    }
    for component in &components {
        if component.is_empty() || *component == "." || *component == ".." {
            return Err(format!("{name:?} is not a plugin name"));
        }
        // One rule that refuses `\`, NUL, spaces and every traversal at once.
        if !component
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(
                "a plugin name is a file name, or folder/file: letters, digits, '.', '_' and \
                 '-' only"
                    .to_owned(),
            );
        }
        // Never option-shaped, even though every path below follows a `--` or sits in `"$1"`.
        if component.starts_with('-') {
            return Err("a plugin name may not start with '-'".to_owned());
        }
    }

    let dir = paths::normalize(dir)
        .map_err(|_| "the configured plugin folder is not an absolute path".to_owned())?;
    let path = paths::normalize(&format!("{dir}/{name}.lua"))
        .map_err(|_| format!("{name:?} is not a plugin name"))?;
    if !std::path::Path::new(&path).starts_with(&dir) {
        return Err(format!("{name:?} is not a plugin name"));
    }
    Ok(path)
}

/// The source a `plugin_write` carries, with exactly one trailing newline so the file is
/// well-formed whatever the model sent, or why there is none to write.
pub fn plugin_source(args: &Args) -> Result<String, String> {
    let source = args
        .source
        .as_deref()
        .ok_or("source is required: send the complete plugin file, not a patch")?;
    if source.trim().is_empty() {
        return Err("source is required and must not be empty".to_owned());
    }
    Ok(format!("{}\n", source.trim_end_matches('\n')))
}

/// The 1-based, inclusive line range a `plugin_read` asked for: the `read_file` shape, so an
/// unbounded read of something enormous is cut to a page rather than sent whole.
pub fn read_range(args: &Args) -> (usize, usize) {
    let start = args.start_line.unwrap_or(1).max(1);
    let end = match args.line_count {
        Some(count) => start.saturating_add(count.max(1)).saturating_sub(1),
        None => start.saturating_add(FILE_TOOL_LINES).saturating_sub(1),
    };
    (start, end)
}

/// Lines `start..=end` of `contents`, as `sed -n 'start,endp'` would print them; empty past the
/// end of the file.
pub fn lines_in_range(contents: &str, start: usize, end: usize) -> String {
    contents
        .lines()
        .skip(start.saturating_sub(1))
        .take(end.saturating_sub(start).saturating_add(1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The plugin names a listing answers with: every `.lua` under `root`, one folder deep, as the
/// name the tools take back (`keys.lua`, `app/ui.lua`) rather than as a path.
pub fn listing_names(root: &str, entries: &[zellij_tile::prelude::FsEntry]) -> Vec<String> {
    let prefix = format!("{}/", root.trim_end_matches('/'));
    let mut names: Vec<String> = entries
        .iter()
        .filter(|entry| entry.path.ends_with(".lua"))
        .filter(|entry| entry.metadata.map(|m| m.is_file).unwrap_or(true))
        .map(|entry| {
            entry
                .path
                .strip_prefix(&prefix)
                .unwrap_or(&entry.path)
                .to_owned()
        })
        .collect();
    names.sort();
    names
}

/// What a `plugin_write` answers once the file is there, and its permissions granted.
pub fn write_report(name: &str, granted: &[PermissionType]) -> String {
    let name = name.trim().trim_end_matches(".lua");
    if granted.is_empty() {
        return format!("wrote {name}.lua; call plugin_reload to load it");
    }
    format!(
        "wrote {name}.lua and granted it {}; call plugin_reload to load it",
        permission_names(granted).join(", ")
    )
}

/// What a `plugin_write` says about a file that is already there.
pub fn exists_report(name: &str) -> String {
    let name = name.trim().trim_end_matches(".lua");
    format!(
        "{name}.lua already exists; pass overwrite=true to replace it, or read it first with \
         plugin_read"
    )
}

/// How many lines of a listing or a search reach the model before the rest is
/// dropped. `read_file` is exempt: its length is what the caller asked for.
pub const FILE_TOOL_LINES: usize = 300;

/// The largest `max_results` a call can ask for. A listing longer than this is
/// truncated by `MAX_TOOL_OUTPUT` anyway, and every line of it is re-sent on
/// every later turn of the conversation.
const MAX_FILE_TOOL_LINES: usize = 2_000;

/// How many lines of this call's output to keep. `None` for `read_file`.
pub fn file_line_cap(name: &str, args: &Args) -> Option<usize> {
    if name == "read_file" {
        return None;
    }
    Some(
        args.max_results
            .unwrap_or(FILE_TOOL_LINES)
            .clamp(1, MAX_FILE_TOOL_LINES),
    )
}

/// Resolve a path the model supplied against the project root, refusing anything
/// that lands outside it.
///
/// A relative path is taken from the root, which is what the schemas tell the
/// model to send; an absolute one is checked as given. The check is lexical and
/// runs before any command does: `normalize` refuses `..` outright, and
/// `contains` compares whole components, so `/work/hocus-old` is not inside
/// `/work/hocus`.
///
/// A symlink inside the project that points out of it is not caught. Resolving
/// one needs the filesystem and the plugin has no access to it — `rg` does not
/// follow symlinks unless asked, and what a link in the user's own checkout
/// reaches is already reachable from the user's own shell.
pub fn resolve_in_root(project: &Project, path: Option<&str>) -> Result<String, String> {
    let path = path.map(str::trim).filter(|path| !path.is_empty());
    let joined = match path {
        None => project.root.clone(),
        Some(path) if std::path::Path::new(path).is_absolute() => path.to_owned(),
        Some(path) => format!("{}/{path}", project.root),
    };
    // The root is absolute and `..`-free by construction, so the only way this
    // fails is the model sending `..` — say that rather than repeating the rule.
    let resolved =
        paths::normalize(&joined).map_err(|_| "path must not contain a .. component".to_owned())?;
    // A path the model gave for a project on this machine, so local by
    // construction — a project's root is a folder here.
    if !projects::contains(project, &Location::local(&resolved)) {
        return Err(format!("path must be inside project {}", project.name));
    }
    Ok(resolved)
}

/// The argv a file call becomes. `target` is the path `resolve_in_root` already
/// checked; nothing else here comes from the model without landing in an
/// argument of its own.
///
/// Zellij execs this directly, so there is no shell to quote for and a value
/// that looks like a flag cannot become one: every path follows a `--`, and
/// every pattern follows the option that consumes it.
pub fn file_argv(name: &str, args: &Args, target: &str) -> Result<Vec<String>, String> {
    match name {
        "find_files" => {
            let pattern = args
                .pattern
                .as_deref()
                .map(str::trim)
                .filter(|pattern| !pattern.is_empty())
                .ok_or("pattern is required and must not be empty")?;
            Ok(vec![
                "rg".to_owned(),
                "--files".to_owned(),
                "--glob".to_owned(),
                glob_for(pattern),
                "--".to_owned(),
                target.to_owned(),
            ])
        },
        "search_files" => {
            let query = args
                .query
                .as_deref()
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .ok_or("query is required and must not be empty")?;
            let mut argv = vec![
                "rg".to_owned(),
                "--line-number".to_owned(),
                "--with-filename".to_owned(),
                "--color".to_owned(),
                "never".to_owned(),
            ];
            if args.case_insensitive {
                argv.push("--ignore-case".to_owned());
            }
            if let Some(glob) = args
                .glob
                .as_deref()
                .map(str::trim)
                .filter(|g| !g.is_empty())
            {
                argv.push("--glob".to_owned());
                argv.push(glob.to_owned());
            }
            // Separated from the path by `-e`, so a query starting with `-` is a
            // pattern rather than an unknown option.
            argv.push("-e".to_owned());
            argv.push(query.to_owned());
            argv.push("--".to_owned());
            argv.push(target.to_owned());
            Ok(argv)
        },
        "read_file" => {
            if args
                .path
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
            {
                return Err("path is required and must not be empty".to_owned());
            }
            // `sed` rather than `cat` even for a whole file: an unbounded read of
            // something enormous would be cut mid-line by MAX_TOOL_OUTPUT after
            // the host had already read all of it.
            let start = args.start_line.unwrap_or(1).max(1);
            let end = match args.line_count {
                Some(count) => start.saturating_add(count.max(1)).saturating_sub(1),
                None => start.saturating_add(FILE_TOOL_LINES).saturating_sub(1),
            };
            Ok(vec![
                "sed".to_owned(),
                "-n".to_owned(),
                format!("{start},{end}p"),
                "--".to_owned(),
                target.to_owned(),
            ])
        },
        other => Err(format!("no such tool: {other}")),
    }
}

/// A Git history call as a fixed argv. Git receives neither an arbitrary
/// subcommand nor an option-shaped revision, and every optional path has
/// already passed `resolve_in_root`.
pub fn git_argv(
    name: &str,
    args: &Args,
    project: &Project,
    target: &str,
) -> Result<Vec<String>, String> {
    let mut argv = vec![
        "git".to_owned(),
        "-C".to_owned(),
        project.root.clone(),
        "--no-pager".to_owned(),
    ];
    match name {
        "git_log" => {
            let max = args.max_results.unwrap_or(100).clamp(1, 500);
            argv.extend([
                "log".to_owned(),
                "--no-color".to_owned(),
                "--date=iso-strict".to_owned(),
                "--format=%H%x09%aI%x09%an%x09%s".to_owned(),
                format!("--max-count={max}"),
            ]);
            if let Some(commit) = args.before_commit.as_deref() {
                argv.push("--skip=1".to_owned());
                argv.push("--end-of-options".to_owned());
                argv.push(commit_id(commit)?.to_owned());
            }
        },
        "git_show" => {
            let commit = args
                .commit
                .as_deref()
                .ok_or_else(|| "commit is required".to_owned())?;
            let commit = commit_id(commit)?;
            argv.extend([
                "show".to_owned(),
                "--no-color".to_owned(),
                "--no-ext-diff".to_owned(),
                "--no-textconv".to_owned(),
                "--format=fuller".to_owned(),
                if args.patch { "--patch" } else { "--no-patch" }.to_owned(),
                "--stat".to_owned(),
                "--end-of-options".to_owned(),
                commit.to_owned(),
            ]);
        },
        other => return Err(format!("no such tool: {other}")),
    }
    argv.push("--".to_owned());
    argv.push(target.to_owned());
    Ok(argv)
}

fn commit_id(commit: &str) -> Result<&str, String> {
    let commit = commit.trim();
    if (4..=64).contains(&commit.len()) && commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(commit)
    } else {
        Err("commit must be a 4-64 character hexadecimal hash from git_log".to_owned())
    }
}

/// Turn the common Git remote spellings into the repository identity `gh`
/// needs. The host is kept so GitHub Enterprise works the same way.
pub fn github_repository(remote: Option<&str>) -> Result<(String, String, String), String> {
    let remote = remote
        .map(str::trim)
        .filter(|remote| !remote.is_empty())
        .ok_or("the active project has no Git remote for github_read")?;
    let (host, path) = if let Some((_, rest)) = remote.split_once("://") {
        let (authority, path) = rest
            .split_once('/')
            .ok_or("the active project's Git remote is not a repository URL")?;
        (authority.rsplit('@').next().unwrap_or(authority), path)
    } else if let Some((authority, path)) = remote.split_once(':') {
        if authority.contains('@') && !authority.contains('/') {
            (authority.rsplit('@').next().unwrap_or(authority), path)
        } else {
            return Err(
                "the active project's Git remote is not a GitHub repository URL".to_owned(),
            );
        }
    } else {
        let (host, path) = remote
            .split_once('/')
            .ok_or("the active project's Git remote is not a GitHub repository URL")?;
        (host, path)
    };
    let parts: Vec<&str> = path
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_matches('/'))
        .split('/')
        .collect();
    if host.is_empty() || parts.len() != 2 || parts.iter().any(|part| part.is_empty()) {
        return Err("the active project's Git remote is not a GitHub repository URL".to_owned());
    }
    Ok((host.to_owned(), parts[0].to_owned(), parts[1].to_owned()))
}

pub fn github_argv(args: &Args, project: &Project) -> Result<Vec<String>, String> {
    let (host, owner, repository) = github_repository(project.remote.as_deref())?;
    let endpoint = args.endpoint.as_deref().unwrap_or_default().trim();
    if endpoint.contains("://") || endpoint.chars().any(char::is_control) {
        return Err("endpoint must be a repository-relative REST path".to_owned());
    }
    let endpoint = endpoint.trim_matches('/');
    let route = endpoint.split('?').next().unwrap_or_default();
    if route.contains('%')
        || route.contains('\\')
        || route
            .split('/')
            .any(|part| matches!(part, "." | "..") || part.is_empty() && !route.is_empty())
    {
        return Err("endpoint must not contain empty, . or .. path components".to_owned());
    }
    let endpoint = if endpoint.is_empty() {
        format!("repos/{owner}/{repository}")
    } else {
        format!("repos/{owner}/{repository}/{endpoint}")
    };
    Ok(vec![
        "gh".to_owned(),
        "api".to_owned(),
        "--method".to_owned(),
        "GET".to_owned(),
        "--hostname".to_owned(),
        host,
        endpoint,
    ])
}

/// A bare name is matched anywhere in the path, a glob is left as written.
///
/// `rg --glob main.rs` matches a file called exactly that at the root of the
/// search and nothing else, so the obvious call finds nothing. Wrapping the
/// names — and only the names — makes `find_files main.rs` mean what it reads as.
fn glob_for(pattern: &str) -> String {
    if pattern.contains(['*', '?', '[', ']', '{', '}', '/']) {
        pattern.to_owned()
    } else {
        format!("*{pattern}*")
    }
}

/// Zellij hands back the stable id of the tab the panes landed in, which is
/// exactly what every Hocus tab tool takes, so it goes straight to the model.
fn moved_panes_result(tab_id: Option<usize>) -> String {
    match tab_id {
        Some(id) => format!("moved to tab_id {id}"),
        None => "zellij did not move the panes; call list_session and check the pane ids and the \
             destination tab"
            .to_owned(),
    }
}

/// What the session looks like, in the only two terms a move cares about.
#[derive(Debug, Default, PartialEq)]
pub struct SessionTabs {
    /// The stable `tab_id` of every tab, in display order.
    pub tabs: Vec<usize>,
    /// Every pane in the session's tabs, tabs flattened.
    pub panes: Vec<PaneId>,
}

fn describe_pane(pane: PaneId) -> String {
    match pane {
        PaneId::Terminal(id) => format!("pane {id}"),
        PaneId::Plugin(id) => format!("plugin pane {id}"),
    }
}

/// `"20"` or `"20%"`, as every geometry argument is written.
///
/// `PercentOrFixed::FromStr` is not enough on its own: it accepts a zero, and its error
/// is a bare "invalid digit found in string" that says nothing about the argument that
/// was wrong. Shared with `floating_geometry` so both tools speak with one voice.
fn percent_or_fixed(name: &str, value: &str, allow_zero: bool) -> Result<PercentOrFixed, String> {
    let (number, is_percent) = match value.strip_suffix('%') {
        Some(number) => (number, true),
        None => (value, false),
    };
    let number = number
        .parse::<usize>()
        .map_err(|_| format!("{name} must be cells like \"20\" or a percentage from 0% to 100%"))?;
    if is_percent && number > 100 {
        return Err(format!("{name} percentage must be from 0% to 100%"));
    }
    if !allow_zero && number == 0 {
        return Err(format!("{name} must be greater than zero"));
    }
    Ok(if is_percent {
        PercentOrFixed::Percent(number)
    } else {
        PercentOrFixed::Fixed(number)
    })
}

/// Refuse a move the session cannot satisfy, naming the argument that is wrong.
///
/// Zellij answers a bad tab or a stale pane id with a bare "did not move", which
/// leaves the model to guess which of its arguments was the problem. The snapshot
/// needed to answer properly is one we already have.
///
/// A tab's display position is not a `tab_id` and shifts whenever a tab closes, so
/// a session that has closed a tab has different values for the two. Passing a
/// position here names a different tab, or none at all — hence the explicit check.
pub fn check_move_to_tab(
    destination: &MoveDestination,
    panes: &[PaneId],
    session: &SessionTabs,
) -> Result<(), String> {
    if let Some(missing) = panes.iter().find(|pane| !session.panes.contains(pane)) {
        return Err(format!(
            "{} is not in this session; call list_session for the current panes",
            describe_pane(*missing)
        ));
    }
    match destination {
        MoveDestination::New(_) => Ok(()),
        MoveDestination::ExistingId(id) => {
            if session.tabs.contains(id) {
                Ok(())
            } else {
                Err(format!(
                    "no tab with tab_id {id} in this session; call list_session for the current \
                     tab_ids (a tab_id is not a position in the tab bar)"
                ))
            }
        },
    }
}

/// The session as `list_session` answers it.
///
/// `SessionInfo` carries a great deal the model has no use for — viewport geometry,
/// tab and pane history, plugin configuration — and every field of it would be paid
/// for in tokens on every subsequent turn of the conversation.
///
/// Pure, and separate from the tool that calls it, so the shape the model reads can be
/// pinned by a test: everything in `mod host` is wasm-only and cannot be run here.
pub fn session_listing(session: &SessionInfo) -> Value {
    let tabs: Vec<Value> = session
        .tabs
        .iter()
        .map(|tab| {
            // PaneManifest is keyed by tab *position*, not by TabInfo.tab_id.
            let panes: Vec<Value> = session
                .panes
                .panes
                .get(&tab.position)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter(|pane| !pane.is_suppressed)
                .map(|pane| {
                    json!({
                        "pane_id": pane.id,
                        "is_plugin": pane.is_plugin,
                        "title": pane.title,
                        "focused": pane.is_focused,
                        "floating": pane.is_floating,
                        "command": pane.terminal_command,
                        "exit_status": pane.exit_status,
                    })
                })
                .collect();
            json!({
                // A label, not an address: this is the number the tab bar
                // shows the user, and it shifts whenever a tab closes.
                // Every tool takes tab_id.
                "tab_number": tab.position + 1,
                "tab_id": tab.tab_id,
                "name": tab.name,
                "active": tab.active,
                "panes": panes,
            })
        })
        .collect();

    json!({ "session": session.name, "tabs": tabs })
}

// ---------------------------------------------------------------------------
// Dispatch — everything below here talks to the host
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod host {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use zellij_tile::prelude::*;

    /// Write a pane's agent record to the session, for every reader of the manifest.
    pub fn report_pane_agent(pane_id: u32, agent: AgentInfo) {
        set_pane_agent(PaneId::Terminal(pane_id), Some(agent));
    }

    /// Write a plugin's source at `path` under `/plugins`, making its folder first. `Ok(false)`
    /// means the file was already there and `overwrite` was not asked for: checked and then
    /// written as two calls, which is the one thing the shell's `set -C` did in one - the folder
    /// has no other writer while a confirmation is open, so the gap is a formality.
    pub fn plugin_fs_write(path: &str, source: &str, overwrite: bool) -> Result<bool, String> {
        let path = std::path::Path::new(path);
        if !overwrite && zellij_tile::shim::fs_metadata(path)?.is_some() {
            return Ok(false);
        }
        zellij_tile::shim::fs_write(path, source.as_bytes(), false, true)?;
        Ok(true)
    }

    /// A plugin's source at `path` under `/plugins`.
    pub fn plugin_fs_read(path: &str) -> Result<String, String> {
        let bytes = zellij_tile::shim::fs_read(std::path::Path::new(path), None)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Every entry under `root`, one folder deep - the depth a module name can reach. A root
    /// that is not there yet is an empty listing: the folder is made by the first write.
    pub fn plugin_fs_list(root: &str) -> Result<Vec<zellij_tile::prelude::FsEntry>, String> {
        let root_path = std::path::Path::new(root);
        if zellij_tile::shim::fs_metadata(root_path)?.is_none() {
            return Ok(Vec::new());
        }
        let mut entries = zellij_tile::shim::fs_list(root_path)?;
        let folders: Vec<String> = entries
            .iter()
            .filter(|entry| entry.metadata.is_some_and(|m| m.is_dir))
            .map(|entry| entry.path.clone())
            .collect();
        for folder in folders {
            entries.extend(zellij_tile::shim::fs_list(std::path::Path::new(&folder))?);
        }
        Ok(entries)
    }

    /// Whether the plugin at `path` under `/plugins` is there to be loaded.
    pub fn plugin_fs_exists(path: &str) -> Result<bool, String> {
        Ok(zellij_tile::shim::fs_metadata(std::path::Path::new(path))?.is_some())
    }

    /// The last `lines` lines of zellij's log about the plugin `name`, read by the host: the
    /// log is outside every root a plugin can reach, and the host holds its path.
    pub fn plugin_log_tail(name: &str, lines: usize) -> Result<String, String> {
        zellij_tile::shim::read_plugin_log(name, lines)
    }

    pub fn dispatch(call: &ToolCall) -> String {
        let args = match Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => return error,
        };
        match call.name.as_str() {
            "list_session" => list_session(),
            "list_other_sessions" => list_other_sessions(),
            "warp_pane" => warp_pane(&args),
            "warp_pane_here" => warp_pane_here(&args),
            "warp_tab" => warp_tab(&args),
            "warp_tab_here" => warp_tab_here(&args),
            "list_orphaned_panes" => list_orphaned_panes_tool(),
            "adopt_orphaned_pane" => adopt_orphaned_pane_tool(&args),
            "read_pane" => with_pane(&args, |pane| read_pane(pane, args.full_scrollback)),
            "send_input" => with_pane(&args, |pane| {
                match args.input_chunks(matches!(pane, PaneId::Terminal(_))) {
                    Ok(chunks) => {
                        for chunk in chunks {
                            write_to_pane_id(chunk, pane);
                        }
                        "input sent".to_owned()
                    },
                    Err(error) => error,
                }
            }),
            "new_pane" => new_pane(&args),
            "new_remote_pane" => new_remote_pane(&args),
            "new_tab" => new_tab_tool(&args),
            "focus" => focus(&args),
            "rename" => rename(&args),
            "move_pane" => with_pane(&args, |pane| move_pane(pane, &args)),
            "swap_panes" => swap_panes_tool(&args),
            "resize_pane" => with_pane(&args, |pane| resize_pane(pane, &args)),
            "toggle_pane" => with_pane(&args, |pane| toggle_pane(pane, &args)),
            "move_panes_to_tab" => move_panes_to_tab(&args),
            "stack_panes" => stack_panes_tool(&args),
            "set_pane_pinned" => with_pane(&args, |pane| {
                let Some(pinned) = args.pinned else {
                    return "pinned is required".to_owned();
                };
                set_floating_pane_pinned(pane, pinned);
                if pinned { "pinned" } else { "unpinned" }.to_owned()
            }),
            "set_floating_pane_geometry" => {
                with_pane(&args, |pane| match args.floating_geometry() {
                    Ok(geometry) => {
                        change_floating_panes_coordinates(vec![(pane, geometry)]);
                        "geometry set".to_owned()
                    },
                    Err(error) => error,
                })
            },
            "rerun_pane" => match args.terminal_pane_id() {
                Ok(id) => {
                    rerun_command_pane(id);
                    "rerun".to_owned()
                },
                Err(error) => error,
            },
            "scroll_pane" => with_pane(&args, |pane| scroll_pane(pane, &args)),
            "clear_pane" => with_pane(&args, |pane| {
                clear_screen_for_pane_id(pane);
                "cleared".to_owned()
            }),
            "close_pane" => with_pane(&args, |pane| {
                close_pane_with_id(pane);
                "closed".to_owned()
            }),
            "close_tab" => match args.tab_target() {
                Ok(id) => {
                    close_tab_with_id(id as u64);
                    "closed".to_owned()
                },
                Err(error) => error,
            },
            other => format!("no such tool: {other}"),
        }
    }

    /// The `(pane_id, is_plugin)` pair is required by most tools and easy for a
    /// model to half-supply, so the check lives in one place.
    fn with_pane(args: &Args, body: impl FnOnce(PaneId) -> String) -> String {
        match (args.pane_id, args.is_plugin) {
            (Some(id), Some(true)) => body(PaneId::Plugin(id)),
            (Some(id), Some(false)) => body(PaneId::Terminal(id)),
            _ => {
                "pane_id and is_plugin are both required; call list_session to get them".to_owned()
            },
        }
    }

    /// The whole session as compact JSON. `SessionInfo` carries a great deal the
    /// model has no use for — viewport geometry, tab and pane history, plugin
    /// configuration — and every field of it would be paid for in tokens on
    /// every subsequent turn of the conversation.
    fn list_session() -> String {
        let snapshot = match get_session_list() {
            Ok(snapshot) => snapshot,
            Err(error) => return format!("could not list sessions: {error}"),
        };
        let Some(session) = snapshot
            .live_sessions
            .iter()
            .find(|session| session.is_current_session)
        else {
            return "the current session is not in the session list".to_owned();
        };
        session_listing(session).to_string()
    }

    /// The other live sessions, terminal panes only.
    ///
    /// A separate tool rather than a flag on `list_session` on purpose.
    /// `list_session` is called constantly and its shape is load-bearing for
    /// every other tool; widening it would make every turn pay for every
    /// session's panes. The name also states the scoping rule, which matters
    /// because the ids in here are only meaningful to the session they came
    /// from.
    ///
    /// Plugin panes are dropped rather than listed and refused later: a plugin
    /// pane has no process to move, so an id the model cannot use is worse than
    /// no id at all.
    fn list_other_sessions() -> String {
        let snapshot = match get_session_list() {
            Ok(snapshot) => snapshot,
            Err(error) => return format!("could not list sessions: {error}"),
        };

        let sessions: Vec<Value> = snapshot
            .live_sessions
            .iter()
            .filter(|session| !session.is_current_session)
            .map(|session| {
                let tabs: Vec<Value> = session
                    .tabs
                    .iter()
                    .map(|tab| {
                        // PaneManifest is keyed by tab *position*, not by TabInfo.tab_id.
                        let panes: Vec<Value> = session
                            .panes
                            .panes
                            .get(&tab.position)
                            .map(Vec::as_slice)
                            .unwrap_or_default()
                            .iter()
                            .filter(|pane| !pane.is_suppressed && !pane.is_plugin)
                            .map(|pane| {
                                json!({
                                    "pane_id": pane.id,
                                    "title": pane.title,
                                    "command": pane.terminal_command,
                                    "exit_status": pane.exit_status,
                                })
                            })
                            .collect();
                        // tab_id is what warp_tab_here takes; tab_number is only
                        // the label that session's tab bar shows.
                        json!({
                            "tab_number": tab.position + 1,
                            "tab_id": tab.tab_id,
                            "name": tab.name,
                            "panes": panes,
                        })
                    })
                    .collect();
                json!({ "session": session.name, "tabs": tabs })
            })
            .collect();

        if sessions.is_empty() {
            return "no other zellij sessions are running".to_owned();
        }
        json!({ "sessions": sessions }).to_string()
    }

    /// Whether a session by this name is currently running, other than ours.
    ///
    /// Checked before the host call because that call blocks the plugin — and
    /// this whole pane, including its UI — for as long as the warp takes. A
    /// session name the model made up is by far the likeliest failure, and it is
    /// answerable from a snapshot we already have.
    fn other_session_exists(name: &str) -> Result<bool, String> {
        let snapshot =
            get_session_list().map_err(|error| format!("could not list sessions: {error}"))?;
        Ok(snapshot
            .live_sessions
            .iter()
            .any(|session| !session.is_current_session && session.name == name))
    }

    fn warp_pane(args: &Args) -> String {
        let Some(to_session) = args.to_session.as_deref() else {
            return "to_session is required; call list_other_sessions for the names".to_owned();
        };
        let placement = match args.placement() {
            Ok(placement) => placement,
            Err(error) => return error,
        };
        match other_session_exists(to_session) {
            Ok(false) => {
                return format!(
                    "no other session named {to_session:?} is running; call list_other_sessions"
                )
            },
            Err(error) => return error,
            Ok(true) => {},
        }
        with_pane(args, |pane| {
            if let PaneId::Plugin(_) = pane {
                // The same refusal the server would give, without spending a
                // round trip to hear it.
                return "this is a plugin pane; it has no process, so there is nothing to move"
                    .to_owned();
            }
            match warp_pane_to_session(pane, to_session, placement) {
                // Both halves are already prose written for whoever asked, and
                // the failures carry their own recovery instructions.
                Ok(message) | Err(message) => message,
            }
        })
    }

    fn warp_pane_here(args: &Args) -> String {
        let Some(from_session) = args.from_session.as_deref() else {
            return "from_session is required; call list_other_sessions for the names".to_owned();
        };
        let Some(pane_id) = args.pane_id else {
            return "pane_id is required; call list_other_sessions for the pane's id in that session"
                .to_owned();
        };
        let placement = match args.placement() {
            Ok(placement) => placement,
            Err(error) => return error,
        };
        match other_session_exists(from_session) {
            Ok(false) => {
                return format!(
                    "no other session named {from_session:?} is running; call list_other_sessions"
                )
            },
            Err(error) => return error,
            Ok(true) => {},
        }
        match warp_pane_from_session(PaneId::Terminal(pane_id), from_session, placement) {
            Ok(message) | Err(message) => message,
        }
    }

    /// The stable `tab_id` of a tab in *this* session, defaulting to whichever
    /// tab is focused when the call named none.
    ///
    /// The warp path is keyed by tab id, not by display position: `Screen.tabs`
    /// is "Keyed by tab.id (stable identifier)" and `check_tab_can_be_warped`
    /// looks the value up there directly. Handing it a position warps whichever
    /// tab happens to own that number as an id — a different tab entirely once
    /// the session has closed one.
    fn resolve_tab_id(target: Option<usize>) -> Result<usize, String> {
        let snapshot =
            get_session_list().map_err(|error| format!("could not list sessions: {error}"))?;
        let session = snapshot
            .live_sessions
            .iter()
            .find(|session| session.is_current_session)
            .ok_or_else(|| "the current session is not in the session list".to_owned())?;
        match target {
            Some(id) => session
                .tabs
                .iter()
                .find(|tab| tab.tab_id == id)
                .map(|tab| tab.tab_id)
                .ok_or_else(|| {
                    format!("no tab with tab_id {id} in this session; call list_session for the current tab_ids, or use warp_tab_here if it came from list_other_sessions")
                }),
            None => session
                .tabs
                .iter()
                .find(|tab| tab.active)
                .map(|tab| tab.tab_id)
                .ok_or_else(|| "no tab is focused".to_owned()),
        }
    }

    fn warp_tab(args: &Args) -> String {
        let Some(to_session) = args.to_session.as_deref() else {
            return "to_session is required; call list_other_sessions for the names".to_owned();
        };
        // Unlike every other tab tool, `warp_tab` reads an absent tab as
        // permission rather than an omission: it means "the tab in focus".
        let tab_id = match resolve_tab_id(args.tab_id) {
            Ok(id) => id,
            Err(error) => return error,
        };
        match other_session_exists(to_session) {
            Ok(false) => {
                return format!(
                    "no other session named {to_session:?} is running; call list_other_sessions"
                )
            },
            Err(error) => return error,
            Ok(true) => {},
        }
        match warp_tab_to_session(tab_id, to_session) {
            Ok(message) | Err(message) => message,
        }
    }

    fn warp_tab_here(args: &Args) -> String {
        let Some(from_session) = args.from_session.as_deref() else {
            return "from_session is required; call list_other_sessions for the names".to_owned();
        };
        let Some(tab_id) = args.tab_id else {
            return "tab_id is required; call list_other_sessions for the tab's tab_id in \
                     that session"
                .to_owned();
        };
        match other_session_exists(from_session) {
            Ok(false) => {
                return format!(
                    "no other session named {from_session:?} is running; call list_other_sessions"
                )
            },
            Err(error) => return error,
            Ok(true) => {},
        }
        match warp_tab_from_session(tab_id, from_session) {
            Ok(message) | Err(message) => message,
        }
    }

    fn list_orphaned_panes_tool() -> String {
        let panes = match list_orphaned_panes() {
            Ok(panes) => panes,
            Err(error) => return format!("could not list orphaned panes: {error}"),
        };
        if panes.is_empty() {
            return "no orphaned panes; nothing has outlived its session".to_owned();
        }
        // `started_at_unix` is a wall clock the model has no way to compare
        // against, so the age is computed here.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let orphans: Vec<Value> = panes
            .iter()
            .map(|pane| {
                json!({
                    "host_id": pane.host_id,
                    "command": pane.command,
                    "cwd": pane.cwd,
                    "session": pane.session_name,
                    "exited": pane.exit_status.is_some(),
                    "exit_code": pane.exit_status.flatten(),
                    "age_seconds": now.saturating_sub(pane.started_at_unix),
                    "buffered_bytes": pane.buffered_bytes,
                })
            })
            .collect();
        json!({ "orphans": orphans }).to_string()
    }

    fn adopt_orphaned_pane_tool(args: &Args) -> String {
        let Some(host_id) = args.host_id.as_deref() else {
            return "host_id is required; call list_orphaned_panes to get one".to_owned();
        };
        let placement = match args.placement() {
            Ok(placement) => placement,
            Err(error) => return error,
        };
        match adopt_orphaned_pane(host_id, placement) {
            Ok(message) | Err(message) => message,
        }
    }

    fn read_pane(pane: PaneId, full: bool) -> String {
        match get_pane_scrollback(pane, full) {
            Ok(contents) => {
                let mut lines = contents.lines_above_viewport;
                lines.extend(contents.viewport);
                lines.extend(contents.lines_below_viewport);
                // Trailing blanks are the unused rows of the pane, not content.
                while lines.last().is_some_and(|line| line.trim().is_empty()) {
                    lines.pop();
                }
                if lines.is_empty() {
                    "(pane is empty)".to_owned()
                } else {
                    lines.join("\n")
                }
            },
            Err(error) => format!("could not read pane: {error}"),
        }
    }

    fn new_pane(args: &Args) -> String {
        new_pane_with_context(args, BTreeMap::new())
    }

    /// `new_pane`, plus the context the pane's originating plugin should carry.
    ///
    /// Only `agent_spawn`'s no-checkout route passes anything: an agent started without a
    /// branch to cut lands here rather than in `spawn_in_checkout`, and the host sets the
    /// agent marker itself only on the git-checkout call. Without this it would open a pane
    /// the host cannot tell from any other command pane, and the `agent_exit` hook would
    /// never fire for it.
    pub fn new_pane_with_context(args: &Args, context: BTreeMap<String, String>) -> String {
        let cwd = args.cwd.clone().map(PathBuf::from);

        // `program_and_args` is what `call_needs_confirmation` read, so a call
        // that was confirmed as running a program runs exactly that program.
        let opened = match args.program_and_args() {
            Some((path, rest)) => {
                let command = CommandToRun {
                    path: PathBuf::from(path),
                    args: rest,
                    cwd: cwd.clone(),
                    ..Default::default()
                };
                if args.in_new_tab {
                    open_command_pane_in_new_tab(command, context).1
                } else if args.floating {
                    open_command_pane_floating(command, None, context)
                } else {
                    open_command_pane(command, context)
                }
            },
            // No command: a plain shell. `open_terminal*` wants a cwd, and the
            // focused pane's is the one the user would expect to land in.
            None => {
                let path = cwd.unwrap_or_else(focused_cwd);
                // A new tab comes with a shell pane already, so opening one
                // inside it would leave the user with two. Report the tab and
                // let the model call list_session if it needs the pane id.
                if args.in_new_tab {
                    return new_tab_tool(&Args {
                        cwd: Some(path.display().to_string()),
                        name: args.name.clone(),
                        ..Args::default()
                    });
                }
                if args.floating {
                    open_terminal_floating(path, None)
                } else {
                    open_terminal(path)
                }
            },
        };

        report_opened_pane(opened, args)
    }

    /// With no command, the far machine's login shell — `new_pane`'s "plain
    /// shell" branch, one machine along.
    ///
    /// Two things differ from that branch, both because the pane is elsewhere.
    /// The cwd is left unset rather than falling back to `focused_cwd()`: that
    /// is a path on *this* machine, and an unset one is precisely what lands the
    /// pane in the remote home directory. And `in_new_tab` still opens a command
    /// pane, where `new_pane` hands off to `new_tab_tool` — the shell a new tab
    /// comes with is a local one, so it is not the pane being asked for.
    fn new_remote_pane(args: &Args) -> String {
        let Some(remote_host) = args.remote_host.clone() else {
            return "remote_host is required".to_owned();
        };
        // The same reading `call_needs_confirmation` used; an empty program name
        // would otherwise reach the far machine as an exec of nothing.
        let command = match args.program_and_args() {
            Some((path, rest)) => CommandToRun {
                path: PathBuf::from(path),
                args: rest,
                cwd: args.cwd.clone().map(PathBuf::from),
                remote_host: Some(remote_host),
                remote_default_shell: false,
            },
            None => CommandToRun {
                // Deliberately empty: which shell the remote user has is a fact
                // about that machine, so the flag asks it rather than guessing.
                path: PathBuf::new(),
                args: vec![],
                cwd: args.cwd.clone().map(PathBuf::from),
                remote_host: Some(remote_host),
                remote_default_shell: true,
            },
        };
        let opened = if args.in_new_tab {
            open_command_pane_in_new_tab(command, BTreeMap::new()).1
        } else if args.floating {
            open_command_pane_floating(command, None, BTreeMap::new())
        } else {
            open_command_pane(command, BTreeMap::new())
        };

        report_opened_pane(opened, args)
    }

    fn report_opened_pane(opened: Option<PaneId>, args: &Args) -> String {
        match opened {
            Some(pane) => {
                if let Some(name) = &args.name {
                    rename_pane_with_id(pane, name);
                }
                let (id, is_plugin) = split_pane_id(pane);
                json!({ "opened": true, "pane_id": id, "is_plugin": is_plugin }).to_string()
            },
            None => "zellij did not open the pane".to_owned(),
        }
    }

    /// Load or reload the plugin at `path`.
    ///
    /// `start_or_reload_plugin` returns nothing, so this cannot report that the plugin loaded;
    /// `observe_plugin`, a moment later, is what does. It *starts* the plugin when it is not
    /// already running, opening a pane, which is the behaviour authoring wants: write it, then
    /// look at it.
    pub fn reload_plugin(path: &str) {
        start_or_reload_plugin(&format!("file:{path}"));
    }

    /// Grant the plugin at `path` the permissions its source asked for, as the user's approval
    /// of the write. The host keys the grant by the same location a reload names.
    pub fn grant_permissions(path: &str, permissions: &[PermissionType]) {
        grant_plugin_permissions(&format!("file:{path}"), permissions);
    }

    /// Find the pane running the plugin at `path` and read what it shows.
    ///
    /// The pane is found by its `plugin_url`, which for a file plugin ends in the path it was
    /// loaded from. Only the current session is searched: a reload from here always lands
    /// there. `None` means no pane is running it - it has not opened yet, or was closed.
    pub fn observe_plugin(path: &str) -> Option<PluginObservation> {
        let snapshot = get_session_list().ok()?;
        let session = snapshot
            .live_sessions
            .iter()
            .find(|session| session.is_current_session)?;
        let pane = session.panes.panes.values().flatten().find(|pane| {
            pane.is_plugin
                && pane
                    .plugin_url
                    .as_deref()
                    .is_some_and(|url| url.ends_with(path))
        })?;
        Some(PluginObservation {
            pane_id: pane.id,
            contents: read_pane(PaneId::Plugin(pane.id), false),
        })
    }

    /// Cut the branch or worktree, then open the agent's pane in it.
    ///
    /// The whole of it is `launch::open`, which is also what the New agent modal's
    /// button calls — the point being that there is one answer to "what does
    /// starting an agent do", not two. All this adds is the reporting: the same
    /// JSON `report_opened_pane` emits, so `present` and `register_spawned`
    /// cannot tell the two `agent_spawn` paths apart, plus the branch and the
    /// folder, which the caller has no other way to learn.
    pub fn spawn_in_checkout(args: &Args, source: Option<String>) -> String {
        // The git work resolves the repository from this folder, so unlike
        // `new_pane` it cannot be left unset. The focused pane's cwd is the one the
        // user would mean, which is the same fallback `new_pane` uses.
        let source = source.map_or_else(focused_cwd, PathBuf::from);
        let spec = args.launch_spec();
        let checkout = match launch::plan(&spec) {
            Ok((_, checkout)) => checkout,
            Err(error) => return error,
        };
        match launch::open(&spec, source) {
            Ok(pane) => {
                if let Some(name) = &args.name {
                    rename_pane_with_id(pane, name);
                }
                let (id, is_plugin) = split_pane_id(pane);
                let mut opened = json!({
                    "opened": true, "pane_id": id, "is_plugin": is_plugin,
                });
                match checkout {
                    GitCheckout::NewWorktree { branch, path, .. } => {
                        opened["branch"] = json!(branch);
                        opened["cwd"] = json!(path.display().to_string());
                    },
                    GitCheckout::NewBranch { branch, .. } => opened["branch"] = json!(branch),
                    GitCheckout::Current => {},
                }
                opened.to_string()
            },
            Err(error) => error,
        }
    }

    fn new_tab_tool(args: &Args) -> String {
        // Both parameters bind the same type parameter, so they must agree.
        let cwd = Some(
            args.cwd
                .clone()
                .unwrap_or_else(|| focused_cwd().display().to_string()),
        );
        match new_tab(args.name.clone(), cwd) {
            // Zellij hands back the stable tab id, not the position. Every tab
            // tool takes tab_id, so there is nothing to resolve: the session
            // list is written to disk once a second and cannot hold the new tab
            // yet anyway.
            Some(tab_id) => json!({ "opened": true, "tab_id": tab_id }).to_string(),
            None => "zellij did not open the tab".to_owned(),
        }
    }

    fn focus(args: &Args) -> String {
        match args.tab_or_pane_target() {
            Some(Ok(id)) => {
                go_to_tab_with_id(id as u64);
                format!("focused tab {id}")
            },
            Some(Err(error)) => error,
            None => with_pane(args, |pane| {
                // Float and un-hide it if it isn't currently visible; focusing a
                // pane the user then can't see would be a confusing no-op.
                focus_pane_with_id(pane, true, false);
                "focused".to_owned()
            }),
        }
    }

    fn rename(args: &Args) -> String {
        let Some(name) = &args.name else {
            return "name is required".to_owned();
        };
        match args.tab_or_pane_target() {
            Some(Ok(id)) => {
                rename_tab_with_id(id as u64, name);
                format!("renamed tab {id}")
            },
            Some(Err(error)) => error,
            None => with_pane(args, |pane| {
                rename_pane_with_id(pane, name);
                "renamed".to_owned()
            }),
        }
    }

    fn move_pane(pane: PaneId, args: &Args) -> String {
        let direction = match PaneDirection::parse(args.direction.as_deref()) {
            Ok(direction) => direction,
            Err(error) => return error,
        };
        if let Some(direction) = direction {
            let panes = match current_pane_positions() {
                Ok(panes) => panes,
                Err(error) => return error,
            };
            if let Err(error) = plan_move(&panes, pane, direction) {
                return error;
            }
            move_pane_with_pane_id_in_direction(pane, host_direction(direction));
        } else {
            move_pane_with_pane_id(pane);
        }
        "moved".to_owned()
    }

    fn swap_panes_tool(args: &Args) -> String {
        let [first, second] = match args.swap_pane_ids() {
            Ok(panes) => panes,
            Err(error) => return error,
        };
        let panes = match current_pane_positions() {
            Ok(panes) => panes,
            Err(error) => return error,
        };
        let direction = match plan_swap(&panes, first, second) {
            Ok(direction) => direction,
            Err(error) => return error,
        };
        move_pane_with_pane_id_in_direction(first, host_direction(direction));
        "swapped".to_owned()
    }

    /// The `PaneId` a manifest entry names. Ids overlap by kind, so the flag is
    /// half of the address rather than a detail.
    fn pane_id_of(pane: &PaneInfo) -> PaneId {
        if pane.is_plugin {
            PaneId::Plugin(pane.id)
        } else {
            PaneId::Terminal(pane.id)
        }
    }

    fn current_pane_positions() -> Result<Vec<PanePosition>, String> {
        let snapshot =
            get_session_list().map_err(|error| format!("could not list sessions: {error}"))?;
        let session = snapshot
            .live_sessions
            .iter()
            .find(|session| session.is_current_session)
            .ok_or_else(|| "the current session is not in the session list".to_owned())?;
        let in_tabs = session.tabs.iter().flat_map(|tab| {
            session
                .panes
                .panes
                .get(&tab.position)
                .into_iter()
                .flatten()
                .map(move |pane| PanePosition {
                    id: pane_id_of(pane),
                    tab_position: Some(tab.position),
                    x: pane.pane_x,
                    y: pane.pane_y,
                    columns: pane.pane_columns,
                    rows: pane.pane_rows,
                    visible: !pane.is_suppressed,
                    selectable: pane.is_selectable,
                    floating: pane.is_floating,
                    fullscreen: tab.is_fullscreen_active,
                    floating_layer_visible: tab.are_floating_panes_visible,
                })
        });
        Ok(in_tabs.collect())
    }

    fn resize_pane(pane: PaneId, args: &Args) -> String {
        let resize = match args.resize.as_deref() {
            Some("increase") => Resize::Increase,
            Some("decrease") => Resize::Decrease,
            _ => return "resize must be \"increase\" or \"decrease\"".to_owned(),
        };
        let direction = match PaneDirection::parse(args.direction.as_deref()) {
            Ok(direction) => direction.map(host_direction),
            Err(error) => return error,
        };
        resize_pane_with_id(ResizeStrategy::new(resize, direction), pane);
        "resized".to_owned()
    }

    fn toggle_pane(pane: PaneId, args: &Args) -> String {
        match args.what.as_deref() {
            Some("fullscreen") => {
                toggle_pane_id_fullscreen(pane);
                "toggled fullscreen".to_owned()
            },
            Some("floating") => {
                toggle_pane_embed_or_eject_for_pane_id(pane);
                "toggled floating".to_owned()
            },
            _ => "what must be \"fullscreen\" or \"floating\"".to_owned(),
        }
    }

    /// The tabs and panes of the current session, for `check_move_to_tab`.
    fn current_session_tabs() -> Result<SessionTabs, String> {
        let snapshot =
            get_session_list().map_err(|error| format!("could not list sessions: {error}"))?;
        let session = snapshot
            .live_sessions
            .iter()
            .find(|session| session.is_current_session)
            .ok_or_else(|| "the current session is not in the session list".to_owned())?;
        Ok(SessionTabs {
            tabs: session.tabs.iter().map(|tab| tab.tab_id).collect(),
            panes: session
                .tabs
                .iter()
                // PaneManifest is keyed by tab *position*, not by TabInfo.tab_id.
                .flat_map(|tab| session.panes.panes.get(&tab.position).into_iter().flatten())
                .map(pane_id_of)
                .collect(),
        })
    }

    fn move_panes_to_tab(args: &Args) -> String {
        let panes = match args.pane_ids(1) {
            Ok(panes) => panes,
            Err(error) => return error,
        };
        let destination = match args.move_destination() {
            Ok(destination) => destination,
            Err(error) => return error,
        };
        // A snapshot we cannot read is not grounds to refuse — fall through and let the
        // host be the judge.
        if let Ok(session) = current_session_tabs() {
            if let Err(error) = check_move_to_tab(&destination, &panes, &session) {
                return error;
            }
        }
        let target = match destination {
            MoveDestination::ExistingId(id) => break_panes_to_tab_with_id(&panes, id, false),
            MoveDestination::New(name) => break_panes_to_new_tab(&panes, name, false),
        };
        moved_panes_result(target)
    }

    fn stack_panes_tool(args: &Args) -> String {
        match args.pane_ids(2) {
            Ok(panes) => {
                stack_panes(panes);
                "stacked".to_owned()
            },
            Err(error) => error,
        }
    }

    fn scroll_pane(pane: PaneId, args: &Args) -> String {
        match args.scroll_action() {
            Ok(ScrollAction::LineUp) => scroll_up_in_pane_id(pane),
            Ok(ScrollAction::LineDown) => scroll_down_in_pane_id(pane),
            Ok(ScrollAction::PageUp) => page_scroll_up_in_pane_id(pane),
            Ok(ScrollAction::PageDown) => page_scroll_down_in_pane_id(pane),
            Ok(ScrollAction::Top) => scroll_to_top_in_pane_id(pane),
            Ok(ScrollAction::Bottom) => scroll_to_bottom_in_pane_id(pane),
            Err(error) => return error,
        }
        "scrolled".to_owned()
    }

    fn host_direction(direction: PaneDirection) -> Direction {
        match direction {
            PaneDirection::Left => Direction::Left,
            PaneDirection::Right => Direction::Right,
            PaneDirection::Up => Direction::Up,
            PaneDirection::Down => Direction::Down,
        }
    }

    fn split_pane_id(pane: PaneId) -> (u32, bool) {
        match pane {
            PaneId::Terminal(id) => (id, false),
            PaneId::Plugin(id) => (id, true),
        }
    }

    /// The cwd a command should default to. The plugin's own cwd is `/host`,
    /// which is almost never what the user means by "here".
    fn focused_cwd() -> PathBuf {
        get_focused_pane_info()
            .ok()
            .and_then(|(_, pane)| get_pane_cwd(pane).ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

#[cfg(target_arch = "wasm32")]
pub use host::{
    dispatch, grant_permissions, new_pane_with_context, observe_plugin, plugin_fs_exists,
    plugin_fs_list, plugin_fs_read, plugin_fs_write, plugin_log_tail, reload_plugin,
    report_pane_agent, spawn_in_checkout,
};

/// What a `plugin_reload` found when it went to look: the plugin's pane, and its text.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginObservation {
    pub pane_id: u32,
    pub contents: String,
}

/// What `plugin_reload` answers, from what `observe_plugin` found.
///
/// The loading screen's error text is the whole of what the host says when a plugin fails to
/// load or raises: it is painted into the pane in place of the plugin. Naming the failure up
/// front is what lets the model act on it without reading the transcript for the word.
pub fn plugin_load_report(path: &str, observed: Option<PluginObservation>) -> String {
    let Some(observed) = observed else {
        return format!(
            "reload requested for {path}, but no pane is running it yet. Call plugin_reload \
             again to look once more, or list_session to see what is open."
        );
    };
    let contents = truncate_str(observed.contents.trim_end(), MAX_TOOL_OUTPUT / 4);
    let failed = contents.contains("failed to load the Lua plugin")
        || contents.contains("failed to run the plugin's")
        || contents.contains("Panic!");
    let verdict = if failed { "FAILED to load" } else { "loaded" };
    format!(
        "{path} {verdict} in plugin pane {} (read_pane with is_plugin=true for more). The pane \
         shows:\n{contents}",
        observed.pane_id
    )
}

/// Native builds exist only to run the unit tests; nothing there dispatches.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch(_call: &ToolCall) -> String {
    "tools are unavailable outside zellij".to_owned()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_in_checkout(_args: &Args, _source: Option<String>) -> String {
    "tools are unavailable outside zellij".to_owned()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn new_pane_with_context(_args: &Args, _context: BTreeMap<String, String>) -> String {
    "tools are unavailable outside zellij".to_owned()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn reload_plugin(_path: &str) {}

/// Off wasm there is no session to hold the record; the tests read `manual_kinds`.
#[cfg(not(target_arch = "wasm32"))]
pub fn report_pane_agent(_pane_id: u32, _agent: zellij_tile::prelude::AgentInfo) {}

/// What every plugin file tool answers off wasm, where there is no host and no plugin folder:
/// the tests cover the pure halves, and this is the sentence the tool loop passes through.
#[cfg(not(target_arch = "wasm32"))]
pub const PLUGIN_FOLDER_UNREACHABLE: &str =
    "the plugin folder is only reachable from inside zellij";

#[cfg(not(target_arch = "wasm32"))]
pub fn plugin_fs_write(_path: &str, _source: &str, _overwrite: bool) -> Result<bool, String> {
    Err(PLUGIN_FOLDER_UNREACHABLE.to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn plugin_fs_read(_path: &str) -> Result<String, String> {
    Err(PLUGIN_FOLDER_UNREACHABLE.to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn plugin_fs_list(_root: &str) -> Result<Vec<zellij_tile::prelude::FsEntry>, String> {
    Err(PLUGIN_FOLDER_UNREACHABLE.to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn plugin_fs_exists(_path: &str) -> Result<bool, String> {
    Err(PLUGIN_FOLDER_UNREACHABLE.to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn plugin_log_tail(_name: &str, _lines: usize) -> Result<String, String> {
    Err(PLUGIN_FOLDER_UNREACHABLE.to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn observe_plugin(_path: &str) -> Option<PluginObservation> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn grant_permissions(_path: &str, _permissions: &[PermissionType]) {}

/// Point at the pane a confirmation is about: bring it into view for whoever is
/// being asked, and write `label` on its frame. Never moves keyboard focus, so
/// the `y` still lands in hocus.
#[cfg(target_arch = "wasm32")]
pub fn raise_spotlight(pane: PaneId, label: String) {
    zellij_tile::shim::reveal_pane_with_id(pane, Some(label));
}

/// Take the mark back off. The reveal is not undone — you are left where the
/// answer was given, which is where the thing you agreed to just happened.
#[cfg(target_arch = "wasm32")]
pub fn drop_spotlight(pane: PaneId) {
    zellij_tile::shim::highlight_and_unhighlight_panes(vec![], vec![pane]);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn raise_spotlight(_pane: PaneId, _label: String) {}

#[cfg(not(target_arch = "wasm32"))]
pub fn drop_spotlight(_pane: PaneId) {}

// ---------------------------------------------------------------------------

#[path = "./unit/tools_tests/mod.rs"]
#[cfg(test)]
mod tools_tests;

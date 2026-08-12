//! The tool set the model uses to drive Zellij.
//!
//! Split in two halves. Everything above the dispatch section is pure and
//! host-testable; `dispatch` and its helpers call the Zellij host and are not.
//! That split is load-bearing rather than stylistic: `host_run_plugin_command`
//! is a no-op on non-wasm32 targets, and most value-returning shims `.unwrap()`
//! the protobuf decode, so calling one from a host-triple test panics.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zellij_tile::prelude::{FloatingPaneCoordinates, PaneId, WarpPlacement};

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

/// Tools that need explicit confirmation before dispatch. All other tools run
/// without asking.
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
            | "adopt_orphaned_pane"
    )
}

/// The one-line transcript label for a call. Arguments are shown as the model
/// wrote them, so a confirmation prompt says exactly what is about to happen.
pub fn summarize(call: &ToolCall) -> String {
    let arguments = call.arguments.trim();
    if arguments.is_empty() || arguments == "{}" {
        call.name.clone()
    } else {
        format!("{} {arguments}", call.name)
    }
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
        "pane_id": { "type": "integer", "description": "Pane id from list_session." },
        "is_plugin": {
            "type": "boolean",
            "description": "The is_plugin flag from list_session. Pane ids are only unique within their kind, so this is required to identify a pane.",
        },
    })
}

/// What can be typed at a pane: literal text, named keys, or both. Shared by
/// `send_input` and `agent_send`, which take the same input and differ only in
/// what they will point it at.
fn input_properties() -> Value {
    json!({
        "text": { "type": "string", "description": "Literal UTF-8 text to send before any keys." },
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
            "description": "Named keys to send after text, in order.",
        },
    })
}

/// Where a warped pane lands in the session receiving it. A closed set, so a
/// placement the model invents is rejected by the schema rather than at dispatch.
fn placement_property() -> Value {
    json!({
        "type": "string",
        "enum": ["new_tab", "focused_tab", "floating"],
        "description": "Where the pane lands in the receiving session. Defaults to new_tab, the only placement that cannot disturb a layout that session already has.",
    })
}

/// How the Slack tools address a conversation and a span of days. Shared by
/// `slack_read` and `slack_backfill`, which take the same window and differ only
/// in whether they read the archive or fill it.
fn slack_properties() -> Value {
    json!({
        "channel": { "type": "string", "description": "Channel name (general or #general), or its id. DMs are addressed by id." },
        "workspace": { "type": "string", "description": "Workspace domain or name. Omit for the only signed-in workspace." },
        "from": { "type": "string", "description": "Inclusive first day, YYYY-MM-DD. Omit for no lower bound." },
        "to": { "type": "string", "description": "Inclusive last day, YYYY-MM-DD. Omit for no upper bound." },
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
            "List every tab and pane in the current Zellij session, with the ids needed by the other tools. Call this first — ids from an earlier turn may be stale.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "list_other_sessions",
            "List the other Zellij sessions running on this machine and the terminal panes in them. Pane ids are per-session, so these are the ids to give warp_pane_here — never the ones from list_session. The data is a snapshot and can be a moment stale.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "warp_pane",
            "Move a pane, and the program running in it, out of this session and into another one. The process keeps running and its scrollback goes with it. Call list_other_sessions first for the target's name. Cannot move a plugin pane, a pane with a scrollback editor open, or this session's last pane.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "to_session": { "type": "string", "description": "Name of the session to move the pane into, from list_other_sessions." },
                    "placement": placement.clone(),
                },
                "required": ["pane_id", "is_plugin", "to_session"],
            }),
        ),
        tool(
            "warp_pane_here",
            "Bring a pane from another session into this one, with its process still running. pane_id is that session's id for the pane, from list_other_sessions — ids from list_session name different panes.",
            json!({
                "type": "object",
                "properties": {
                    "from_session": { "type": "string", "description": "Name of the session the pane is currently in, from list_other_sessions." },
                    "pane_id": { "type": "integer", "description": "The pane's id in from_session, from list_other_sessions." },
                    "placement": placement.clone(),
                },
                "required": ["from_session", "pane_id"],
            }),
        ),
        tool(
            "list_orphaned_panes",
            "List panes whose session died but whose process is still running, waiting to be recovered. Empty unless a Zellij server was killed. Panes in other live sessions are not listed here — those move with warp_pane_here.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "adopt_orphaned_pane",
            "Take an orphaned pane into this session, reconnecting it to its still-running process along with whatever it printed while orphaned. host_id comes from list_orphaned_panes.",
            json!({
                "type": "object",
                "properties": {
                    "host_id": { "type": "string", "description": "The orphaned pane's host_id, from list_orphaned_panes." },
                    "placement": placement.clone(),
                },
                "required": ["host_id"],
            }),
        ),
        tool(
            "read_pane",
            "Read the text currently displayed in a pane. Use this to see command output, errors, or what a program is showing.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "full_scrollback": {
                        "type": "boolean",
                        "description": "Include scrollback above and below the visible viewport. Defaults to false; can be very long.",
                    },
                },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "send_input",
            "Send literal text and/or named keys to a pane. Text is sent first, followed by keys in order. Use this to interact with a program already running in a pane.",
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
            "Open a new pane. With `command`, the pane runs that command and the user can re-run it and see its exit status. Without one, it is a normal shell.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Program to run. Omit for a plain shell pane." },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "Arguments to `command`." },
                    "cwd": { "type": "string", "description": "Working directory. Defaults to the focused pane's." },
                    "name": { "type": "string", "description": "Title for the new pane." },
                    "floating": { "type": "boolean", "description": "Open as a floating pane instead of tiled." },
                    "in_new_tab": { "type": "boolean", "description": "Open in a new tab instead of the current one." },
                },
            }),
        ),
        tool(
            "new_tab",
            "Open a new tab.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Title for the new tab." },
                    "cwd": { "type": "string", "description": "Working directory for the tab's first pane." },
                },
            }),
        ),
        tool(
            "focus",
            "Focus a pane or switch to a tab. Give exactly one of pane pair, tab_index, or tab_id.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "tab_index": { "type": "integer", "description": "0-based tab position from list_session." },
                    "tab_id": { "type": "integer", "description": "Stable tab id from list_session. Prefer over tab_index; avoids stale-position errors after close/reorder." },
                },
            }),
        ),
        tool(
            "rename",
            "Rename a pane or a tab. Give exactly one of pane pair, tab_index, or tab_id.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "tab_index": { "type": "integer", "description": "0-based tab position from list_session." },
                    "tab_id": { "type": "integer", "description": "Stable tab id from list_session. Prefer over tab_index." },
                    "name": { "type": "string", "description": "The new name." },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "move_pane",
            "Move one visible tiled pane by swapping it with its unique neighbor in a direction. Omit direction only to use Zellij's next layout position. Use swap_panes when the user names both panes, and set_floating_pane_geometry for floating panes. Call once and trust a successful result.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "is_plugin": pane["is_plugin"],
                    "direction": {
                        "type": "string",
                        "enum": ["left", "right", "up", "down"],
                        "description": "Direction of the unique adjacent tiled pane to swap with. Omit only for Zellij's next layout position.",
                    },
                },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "swap_panes",
            "Swap exactly two visible tiled panes that uniquely share an edge. Use this when the user names both panes; call it once and trust a successful result.",
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
                        "description": "Which edge to move. Omit to resize on all sides.",
                    },
                },
                "required": ["pane_id", "is_plugin", "resize"],
            }),
        ),
        tool(
            "toggle_pane",
            "Toggle a pane between fullscreen and normal, or between floating and tiled.",
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
            "Move one or more panes to an existing tab or a new tab. Set exactly one of tab_index, tab_id, or new_tab=true.",
            json!({
                "type": "object",
                "properties": {
                    "panes": { "type": "array", "items": pane_ref, "minItems": 1 },
                    "tab_index": { "type": "integer", "description": "Existing 0-based tab position from list_session." },
                    "tab_id": { "type": "integer", "description": "Stable tab id from list_session. Prefer over tab_index." },
                    "new_tab": { "type": "boolean", "description": "Set true to create a new tab instead of targeting tab_index/tab_id." },
                    "new_tab_name": { "type": "string", "description": "Optional name when new_tab is true." },
                },
                "required": ["panes"],
            }),
        ),
        tool(
            "stack_panes",
            "Stack two or more panes together in the current layout.",
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
            "Set whether a floating pane is pinned.",
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
            "Set a floating pane's position or size. Values are cells such as \"20\" or percentages such as \"50%\".",
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
            "Re-run the command in a terminal command pane.",
            json!({
                "type": "object",
                "properties": { "pane_id": pane["pane_id"], "is_plugin": pane["is_plugin"] },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "scroll_pane",
            "Scroll a pane's viewport.",
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
            "Clear a pane's scrollback.",
            json!({
                "type": "object",
                "properties": { "pane_id": pane["pane_id"], "is_plugin": pane["is_plugin"] },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "close_pane",
            "Close a pane. This kills whatever is running in it.",
            json!({
                "type": "object",
                "properties": { "pane_id": pane["pane_id"], "is_plugin": pane["is_plugin"] },
                "required": ["pane_id", "is_plugin"],
            }),
        ),
        tool(
            "close_tab",
            "Close a tab and every pane in it. Prefer tab_id over tab_index.",
            json!({
                "type": "object",
                "properties": {
                    "tab_index": { "type": "integer", "description": "0-based tab position from list_session." },
                    "tab_id": { "type": "integer", "description": "Stable tab id from list_session. Prefer over tab_index." },
                },
            }),
        ),
        tool(
            "web_search",
            "Search the web and get back titles, URLs and snippets. Use it for anything you would otherwise be guessing at — current events, release notes, error messages, documentation. Snippets only: there is no way to fetch a page's full text.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to search for, as you would type it into a search engine." },
                },
                "required": ["query"],
            }),
        ),
        tool(
            "schedule",
            "Repeat a prompt in this conversation on an interval, so you can watch something over time — e.g. read a pane every minute and report when it changes. The prompt is asked again as if the user had typed it, with the whole conversation so far still in view, so it can refer to what you saw last run. Repeats until cancelled: call unschedule as soon as the user says stop or the thing being watched has happened.",
            json!({
                "type": "object",
                "properties": {
                    "seconds": { "type": "number", "description": "Seconds between runs." },
                    "prompt": { "type": "string", "description": "What to do each run, written as an instruction to yourself." },
                    "show": { "type": "boolean", "description": "Pop the hocus pane open when this job runs. Use it when the user asked to be told about something; leave it off for quiet background watching." },
                    "auto_confirm": { "type": "boolean", "description": "Run confirmation-gated tools without asking during this job. Creating the first unrestricted job requires one confirmation. Defaults to false." },
                },
                "required": ["seconds", "prompt"],
            }),
        ),
        tool(
            "unschedule",
            "Cancel a scheduled job. Omit job_id to cancel all of them.",
            json!({
                "type": "object",
                "properties": {
                    "job_id": { "type": "integer", "description": "Job id from schedule. Omit to cancel every job." },
                },
            }),
        ),
        tool(
            "project_list",
            "List remembered projects, their roots, Git remotes and worktree status, assigned tabs, and the active project. Includes the projects hocus found by itself from the Git checkouts agents run in: discovered marks these, and auto_tab_ids gives the tabs it worked out rather than the user. Prefer a name the user gave over one hocus made up.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "project_assign",
            "Assign a tab to a project. Give root when the project is new or its root changed. Omit tab_id for the active tab.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Exact project name." },
                    "root": { "type": "string", "description": "Absolute project root. Required for a new project." },
                    "tab_id": { "type": "integer", "description": "Stable tab id. Defaults to the active tab." },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "project_forget",
            "Forget a project. The todo list of its folder stays and comes back with the project. A project hocus discovered by itself is made again by the next poll while an agent is still running in its checkout, so forget one of those only once the agent is gone.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Exact project name from project_list." },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "project_open",
            "Open and assign a new tab at a project root. Give root when the project is new or its root changed.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Exact project name." },
                    "root": { "type": "string", "description": "Absolute project root. Required for a new project." },
                    "tab_name": { "type": "string", "description": "Tab name. Defaults to the project name." },
                },
                "required": ["name"],
            }),
        ),
        // ponytail: `parse_command` splits key=value pairs on whitespace, so a
        // typed `/todo_add text=fix the bug` cannot work. The form that does is
        // `/todo_add {"text": "fix the bug"}`.
        tool(
            "todo_add",
            "Add an item to a project's todo list. The list belongs to the project folder and lasts across sessions. Omit project for the active project. Give parent_id from todo_list to put the item under another item of the same project.",
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The item, in the user's own words. Up to 500 characters." },
                    "project": { "type": "string", "description": "Exact project name from project_list, or an absolute project root. Defaults to the active project." },
                    "parent_id": { "type": "integer", "description": "Item id from todo_list to nest under. The item must be in the same project. Items nest 5 levels deep at most." },
                },
                "required": ["text"],
            }),
        ),
        tool(
            "todo_list",
            "Read a project's todo list as a tree. Every item carries an id for todo_done, at every depth. Items already crossed off are hidden unless include_done is true.",
            json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Exact project name from project_list, or an absolute project root. Defaults to the active project." },
                    "include_done": { "type": "boolean", "description": "Show the items already crossed off. Defaults to false." },
                },
            }),
        ),
        tool(
            "todo_search",
            "Find todo items whose text holds the query. The match ignores case. Each result names the project and the items above the match, so you can say where it is. An item already crossed off matches too, and says so.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Words to look for in the item text." },
                    "project": { "type": "string", "description": "Exact project name from project_list, or an absolute project root. Defaults to the active project." },
                    "all_projects": { "type": "boolean", "description": "Search every project's list. Overrides project. Defaults to false." },
                },
                "required": ["query"],
            }),
        ),
        tool(
            "todo_done",
            "Cross a todo item off. The items below it are crossed off with it. The item stays on the list. Give undo to open an item again: an undo opens the items above it and leaves the items below it crossed off.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Item id from todo_list or todo_search. An id is unique in one project at any depth." },
                    "project": { "type": "string", "description": "Exact project name from project_list, or an absolute project root. Defaults to the active project." },
                    "undo": { "type": "boolean", "description": "Open the item again instead of crossing it off. Defaults to false." },
                },
                "required": ["id"],
            }),
        ),
        tool(
            "agent_register",
            "Mark a terminal pane as an agent, so the agent tools will act on it. Use this for a pane agent_list did not find by itself — a shell you are treating as an agent, or a tool hocus does not recognise. The kind is a label of your choosing and replaces any detected one.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "kind": {
                        "type": "string",
                        "description": "What to call this agent, e.g. codex or reviewer. Lowercase letters, digits, _ and -, up to 32 characters.",
                    },
                },
                "required": ["pane_id", "kind"],
            }),
        ),
        tool(
            "agent_list",
            "List agent panes, optionally for one project. Each agent gives the folder it runs in and the project that owns that folder. Without a project, use the active project or list the full session.",
            json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Exact project name from project_list." },
                },
            }),
        ),
        tool(
            "agent_send",
            "Send literal text and/or named keys to an agent pane. Same as send_input, but it refuses any pane that is not an agent. Prompts usually need keys ['enter'] after the text to be submitted.",
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
            "Interrupt, restart or close an agent. Use interrupt to stop what it is doing without losing the pane, restart to re-run a finished command pane, and close to kill it. To bring an agent into view, use focus instead.",
            json!({
                "type": "object",
                "properties": {
                    "pane_id": pane["pane_id"],
                    "action": {
                        "type": "string",
                        "enum": ["interrupt", "restart", "close"],
                        "description": "interrupt sends ctrl_c; restart re-runs a command pane; close kills the pane.",
                    },
                },
                "required": ["pane_id", "action"],
            }),
        ),
        tool(
            "agent_spawn",
            "Open a new tiled terminal pane running an agent command, in view of the user, and register it as an agent. Use agent_list then agent_send to drive it. For a floating pane, a new tab or a plain shell, use new_pane instead.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The agent program to run, e.g. claude." },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "Arguments to `command`." },
                    "cwd": { "type": "string", "description": "Working directory. Defaults to the focused pane's." },
                    "project": { "type": "string", "description": "Exact project name. Defaults to the active project." },
                    "kind": {
                        "type": "string",
                        "description": "Label for the agent when the command is not one hocus detects. Same rules as agent_register.",
                    },
                },
                "required": ["command"],
            }),
        ),
        tool(
            "slack_status",
            "Report the Slack archive: where it is, how many messages and channels it holds, its date range, the busiest channels, and whether a daemon is running to keep it filled. Call this first whenever another Slack tool fails or returns nothing — it distinguishes a missing archive from a stopped daemon from a channel that was simply never watched, which is otherwise guesswork.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "slack_search",
            "Search the local Slack archive and get back matching messages with their time, channel and author. This is SQLite full-text search over what dobbs has archived, not Slack's own search: the query is FTS5 syntax, so bare words are ANDed, \"a phrase\" is quoted, and NOT/OR work. Use it to find where something was discussed, then slack_read that channel for the surrounding conversation.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "FTS5 query, e.g. deploy NOT staging." },
                },
                "required": ["query"],
            }),
        ),
        tool(
            "slack_read",
            "Read a Slack channel as a transcript. This is the whole conversation, so answer or summarise from it directly in your next message — there is no separate summarise tool and re-reading gets you the same text. Narrow busy channels with from/to rather than pulling everything. Only what has been archived is here: an empty transcript means the channel was never watched or backfilled, so call slack_backfill for it and read again.",
            json!({
                "type": "object",
                "properties": slack_properties(),
                "required": ["channel"],
            }),
        ),
        tool(
            "slack_backfill",
            "Fetch a channel's history and thread replies from Slack into the local archive, so slack_search and slack_read can see it. Use it when slack_read comes back empty or obviously starts too late. It talks to Slack and is rate-limited, so give it a from/to window rather than asking for all of history.",
            json!({
                "type": "object",
                "properties": slack_properties(),
                "required": ["channel"],
            }),
        ),
        tool(
            "slack_post",
            "Post a message to Slack as the user. Sends immediately and cannot be edited or unsent from here, so write it as the user would. To reply inside a thread, pass thread; without it the message goes to the channel where everyone sees it.",
            json!({
                "type": "object",
                "properties": {
                    "channel": slack["channel"],
                    "workspace": slack["workspace"],
                    "text": { "type": "string", "description": "The message to post." },
                    "thread": { "type": "string", "description": "Timestamp of the message to reply under, as shown in a slack_read transcript." },
                },
                "required": ["channel", "text"],
            }),
        ),
    ])
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
    Existing(usize),
    ExistingId(usize),
    New(Option<String>),
}

#[derive(Debug, PartialEq)]
pub enum TabTarget {
    Index(usize),
    Id(usize),
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
    tab_index: usize,
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
    if first.tab_index != second.tab_index {
        return Err("swap_panes needs both panes in the same tab".to_owned());
    }
    if !first.can_move() || !second.can_move() {
        return Err("swap_panes only supports visible selectable tiled panes".to_owned());
    }
    let direction = adjacent_direction(first, second)
        .ok_or_else(|| "panes are not adjacent; do not retry with move_pane".to_owned())?;
    let neighbors = panes
        .iter()
        .filter(|pane| pane.id != first.id && pane.tab_index == first.tab_index && pane.can_move())
        .filter(|pane| adjacent_direction(first, pane) == Some(direction))
        .count();
    if neighbors != 1 {
        return Err("the requested edge has multiple adjacent panes; swap is ambiguous".to_owned());
    }
    let opposite = adjacent_direction(second, first).expect("adjacency is symmetric");
    let reverse_neighbors = panes
        .iter()
        .filter(|pane| {
            pane.id != second.id && pane.tab_index == second.tab_index && pane.can_move()
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
            pane.id != source.id && pane.tab_index == source.tab_index && pane.can_move()
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
    pub tab_index: Option<usize>,
    pub tab_id: Option<usize>,
    pub new_tab: bool,
    pub new_tab_name: Option<String>,
    pub panes: Vec<PaneRef>,
    pub name: Option<String>,
    pub cwd: Option<String>,
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
        match (self.tab_index, self.tab_id, self.new_tab) {
            (Some(index), None, false) if self.new_tab_name.is_none() => {
                Ok(MoveDestination::Existing(index))
            },
            (None, Some(id), false) if self.new_tab_name.is_none() => {
                Ok(MoveDestination::ExistingId(id))
            },
            (None, None, true) => Ok(MoveDestination::New(self.new_tab_name.clone())),
            _ => Err("set exactly one of tab_index, tab_id, or new_tab=true; new_tab_name is only valid for a new tab".to_owned()),
        }
    }

    pub fn tab_target(&self) -> Result<TabTarget, String> {
        match (self.tab_index, self.tab_id) {
            (Some(index), None) => Ok(TabTarget::Index(index)),
            (None, Some(id)) => Ok(TabTarget::Id(id)),
            (None, None) => {
                Err("tab_index or tab_id is required; call list_session to get them".to_owned())
            },
            (Some(_), Some(_)) => {
                Err("give exactly one of tab_index or tab_id, not both".to_owned())
            },
        }
    }

    /// `focus` and `rename` take either a tab or a pane pair. `None` means the
    /// call named no tab at all, so the caller falls through to the pane path.
    pub fn tab_or_pane_target(&self) -> Option<Result<TabTarget, String>> {
        if self.tab_index.is_none() && self.tab_id.is_none() {
            return None;
        }
        Some(match self.tab_target() {
            Ok(_) if self.pane_id.is_some() || self.is_plugin.is_some() => {
                Err("give exactly one of pane pair, tab_index, or tab_id".to_owned())
            },
            target => target,
        })
    }

    pub fn floating_geometry(&self) -> Result<FloatingPaneCoordinates, String> {
        fn validate(name: &str, value: &Option<String>, allow_zero: bool) -> Result<(), String> {
            let Some(value) = value else { return Ok(()) };
            let (number, is_percent) = match value.strip_suffix('%') {
                Some(number) => (number, true),
                None => (value.as_str(), false),
            };
            let number = number.parse::<usize>().map_err(|_| {
                format!("{name} must be cells like \"20\" or a percentage from 0% to 100%")
            })?;
            if is_percent && number > 100 {
                return Err(format!("{name} percentage must be from 0% to 100%"));
            }
            if !allow_zero && number == 0 {
                return Err(format!("{name} must be greater than zero"));
            }
            Ok(())
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

/// Zellij returns a stable tab id here, while every Hocus tab tool consumes the
/// tab's current 0-based position. Do not hand the model an incompatible id.
fn moved_panes_result(tab_id: Option<usize>) -> String {
    match tab_id {
        Some(_) => "moved; call list_session for the tab's current index".to_owned(),
        None => "zellij did not move the panes".to_owned(),
    }
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
                Ok(TabTarget::Index(index)) => {
                    close_tab_with_index(index);
                    "closed".to_owned()
                },
                Ok(TabTarget::Id(id)) => {
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
                    "tab_index": tab.position,
                    "tab_id": tab.tab_id,
                    "name": tab.name,
                    "active": tab.active,
                    "panes": panes,
                })
            })
            .collect();

        json!({ "session": session.name, "tabs": tabs }).to_string()
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
                        json!({ "name": tab.name, "panes": panes })
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
        let words = args.command_words();
        let cwd = args.cwd.clone().map(PathBuf::from);

        let opened = match words.split_first() {
            Some((path, rest)) => {
                let command = CommandToRun {
                    path: PathBuf::from(path),
                    args: rest.to_vec(),
                    cwd: cwd.clone(),
                };
                if args.in_new_tab {
                    open_command_pane_in_new_tab(command, BTreeMap::new()).1
                } else if args.floating {
                    open_command_pane_floating(command, None, BTreeMap::new())
                } else {
                    open_command_pane(command, BTreeMap::new())
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
            Some(Ok(TabTarget::Index(index))) => {
                go_to_tab(index as u32);
                format!("focused tab {index}")
            },
            Some(Ok(TabTarget::Id(id))) => {
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
            Some(Ok(TabTarget::Index(index))) => {
                // Work around Zellij bug: PluginCommand::RenameTab expects 1-based index
                // (see Zellij screen.rs RenameTab saturating_sub(1)), while go_to_tab
                // correctly adds +1 server-side. Until Zellij is fixed, compensate here.
                // The stable tab_id path avoids this entirely.
                rename_tab((index as u32).wrapping_add(1), name);
                format!("renamed tab {index}")
            },
            Some(Ok(TabTarget::Id(id))) => {
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

    fn current_pane_positions() -> Result<Vec<PanePosition>, String> {
        let snapshot =
            get_session_list().map_err(|error| format!("could not list sessions: {error}"))?;
        let session = snapshot
            .live_sessions
            .iter()
            .find(|session| session.is_current_session)
            .ok_or_else(|| "the current session is not in the session list".to_owned())?;
        Ok(session
            .tabs
            .iter()
            .flat_map(|tab| {
                session
                    .panes
                    .panes
                    .get(&tab.position)
                    .into_iter()
                    .flatten()
                    .map(move |pane| PanePosition {
                        id: if pane.is_plugin {
                            PaneId::Plugin(pane.id)
                        } else {
                            PaneId::Terminal(pane.id)
                        },
                        tab_index: tab.position,
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
            })
            .collect())
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

    fn move_panes_to_tab(args: &Args) -> String {
        let panes = match args.pane_ids(1) {
            Ok(panes) => panes,
            Err(error) => return error,
        };
        let target = match args.move_destination() {
            Ok(MoveDestination::Existing(index)) => {
                break_panes_to_tab_with_index(&panes, index, false)
            },
            Ok(MoveDestination::ExistingId(id)) => break_panes_to_tab_with_id(&panes, id, false),
            Ok(MoveDestination::New(name)) => break_panes_to_new_tab(&panes, name, false),
            Err(error) => return error,
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
pub use host::dispatch;

/// Native builds exist only to run the unit tests; nothing there dispatches.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch(_call: &ToolCall) -> String {
    "tools are unavailable outside zellij".to_owned()
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_policy_lists_tools_correctly() {
        for name in [
            "rerun_pane",
            "clear_pane",
            "send_input",
            "agent_send",
            "agent_control",
            // Forgetting a project is the one project tool that takes
            // something away.
            "project_forget",
            "slack_post",
            // Moving a pane takes it off the screen, and briefly leaves it
            // belonging to no session at all.
            "warp_pane",
            "warp_pane_here",
            "adopt_orphaned_pane",
        ] {
            assert!(needs_confirmation(name), "{name} should be confirmed");
        }
        for name in [
            "list_session",
            // Listing is always free, including the two that look outward.
            "list_other_sessions",
            "list_orphaned_panes",
            "read_pane",
            "new_pane",
            "new_tab",
            "focus",
            "rename",
            "move_pane",
            "swap_panes",
            "resize_pane",
            "toggle_pane",
            "move_panes_to_tab",
            "stack_panes",
            "set_pane_pinned",
            "set_floating_pane_geometry",
            "scroll_pane",
            "web_search",
            "schedule",
            "unschedule",
            "agent_register",
            "agent_list",
            "close_pane",
            "close_tab",
            // A todo tool only writes to a file of the user's own notes.
            "todo_add",
            "todo_list",
            "todo_search",
            "todo_done",
            // Reading Slack is free; only posting stops for a y/n.
            "slack_status",
            "slack_search",
            "slack_read",
            "slack_backfill",
        ] {
            assert!(!needs_confirmation(name), "{name} should not be confirmed");
        }
    }

    fn argv(name: &str, arguments: &str) -> Result<Vec<String>, String> {
        slack_argv(name, &Args::parse(arguments).unwrap())
    }

    #[test]
    fn slack_argv_maps_each_tool_onto_its_dobbs_command() {
        assert_eq!(argv("slack_status", "{}").unwrap(), ["status"]);
        assert_eq!(
            argv("slack_search", r#"{"query": "deploy NOT staging"}"#).unwrap(),
            ["search", "deploy NOT staging"]
        );
        // A channel name is passed through as written; dobbs resolves it.
        assert_eq!(
            argv(
                "slack_read",
                r##"{"channel": "#general", "from": "2026-08-01"}"##
            )
            .unwrap(),
            [
                "export-channel",
                "-channel",
                "#general",
                "-from",
                "2026-08-01"
            ]
        );
        assert_eq!(
            argv("slack_backfill", r#"{"channel": "C1", "to": "2026-08-08"}"#).unwrap(),
            ["backfill-channel", "-channel", "C1", "-to", "2026-08-08"]
        );
        assert_eq!(
            argv(
                "slack_post",
                r#"{"channel": "C1", "text": "hi", "thread": "1.2"}"#
            )
            .unwrap(),
            ["post", "-channel", "C1", "-text", "hi", "-thread", "1.2"]
        );
    }

    /// An absent optional flag must be left out entirely rather than passed
    /// empty: dobbs reads `-from ""` as open, but `-channel ""` as an error.
    #[test]
    fn slack_argv_omits_absent_and_blank_options() {
        assert_eq!(
            argv(
                "slack_read",
                r#"{"channel": "eng", "workspace": "  ", "to": ""}"#
            )
            .unwrap(),
            ["export-channel", "-channel", "eng"]
        );
    }

    #[test]
    fn slack_argv_rejects_missing_required_arguments() {
        for (name, arguments, field) in [
            ("slack_search", "{}", "query"),
            ("slack_read", r#"{"from": "2026-08-01"}"#, "channel"),
            ("slack_backfill", "{}", "channel"),
            ("slack_post", r#"{"text": "hi"}"#, "channel"),
            ("slack_post", r#"{"channel": "C1", "text": "  "}"#, "text"),
        ] {
            let error = argv(name, arguments).unwrap_err();
            assert!(error.starts_with(field), "{name}: {error}");
        }
        assert!(argv("slack_nope", "{}")
            .unwrap_err()
            .contains("no such tool"));
    }

    /// The four Slack tools are intercepted in `run_head` by name, so the list
    /// and the advertised schemas have to stay in step.
    #[test]
    fn every_slack_tool_is_advertised() {
        let specs = specs();
        let names: Vec<&str> = specs
            .as_array()
            .unwrap()
            .iter()
            .map(|spec| spec["function"]["name"].as_str().unwrap())
            .collect();
        for name in SLACK_TOOLS {
            assert!(names.contains(&name), "{name} is not advertised");
        }
    }

    #[test]
    fn schedule_advertises_optional_automatic_confirmation() {
        let specs = specs();
        let schedule = specs
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["function"]["name"] == "schedule")
            .unwrap();
        let field = &schedule["function"]["parameters"]["properties"]["auto_confirm"];

        assert_eq!(field["type"], "boolean");
        assert!(!schedule["function"]["parameters"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "auto_confirm"));
    }

    /// The confirmation prompt is the only thing standing between the model and
    /// a closed pane, so every advertised tool must be in the table above.
    #[test]
    fn every_advertised_tool_is_covered_by_the_confirmation_table() {
        let specs = specs();
        let names: Vec<&str> = specs
            .as_array()
            .unwrap()
            .iter()
            .map(|spec| spec["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 46);
        let confirmed = names.iter().filter(|name| needs_confirmation(name)).count();
        assert_eq!(confirmed, 11);
    }

    /// `agent_control` is the one tool whose argument is a verb rather than a target,
    /// so the set of verbs has to be closed at the schema, not at dispatch.
    #[test]
    fn agent_control_schema_has_a_closed_action_set() {
        let specs = specs();
        let control = specs
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["function"]["name"] == "agent_control")
            .unwrap();
        let parameters = &control["function"]["parameters"];
        assert_eq!(
            parameters["properties"]["action"]["enum"],
            json!(["interrupt", "restart", "close"]),
        );
        assert_eq!(parameters["required"], json!(["pane_id", "action"]));
    }

    /// An agent is always a terminal pane, so no agent tool takes `is_plugin` — and
    /// `agent_send` takes either input form alone, so neither is required.
    #[test]
    fn agent_schemas_require_a_pane_and_never_a_pane_kind() {
        let specs = specs();
        for (name, required) in [
            ("agent_register", json!(["pane_id", "kind"])),
            ("agent_list", Value::Null),
            ("agent_send", json!(["pane_id"])),
            ("agent_control", json!(["pane_id", "action"])),
            // The one agent tool with no pane to name: its pane does not exist yet.
            ("agent_spawn", json!(["command"])),
        ] {
            let parameters = specs
                .as_array()
                .unwrap()
                .iter()
                .find(|spec| spec["function"]["name"] == name)
                .unwrap_or_else(|| panic!("{name} is not advertised"))["function"]["parameters"]
                .clone();
            assert_eq!(parameters["required"], required, "{name}");
            assert!(parameters["properties"]["is_plugin"].is_null(), "{name}");
        }
    }

    #[test]
    fn every_spec_has_a_description_and_an_object_schema() {
        for spec in specs().as_array().unwrap() {
            let function = &spec["function"];
            assert_eq!(spec["type"], "function");
            assert!(function["name"].is_string());
            assert!(!function["description"].as_str().unwrap().is_empty());
            assert_eq!(function["parameters"]["type"], "object");
        }
    }

    #[test]
    fn swap_panes_schema_requires_exactly_two_complete_pane_refs() {
        let specs = specs();
        let swap = specs
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["function"]["name"] == "swap_panes")
            .unwrap();
        let panes = &swap["function"]["parameters"]["properties"]["panes"];
        assert_eq!(swap["function"]["parameters"]["required"], json!(["panes"]));
        assert_eq!(panes["minItems"], 2);
        assert_eq!(panes["maxItems"], 2);
        assert_eq!(panes["items"]["required"], json!(["pane_id", "is_plugin"]));
    }

    /// An open string here would let the model invent `"float"` and only find
    /// out at dispatch; a closed enum makes the provider reject it first.
    #[test]
    fn every_warp_schema_has_a_closed_placement_set() {
        let specs = specs();
        for name in ["warp_pane", "warp_pane_here", "adopt_orphaned_pane"] {
            let spec = specs
                .as_array()
                .unwrap()
                .iter()
                .find(|spec| spec["function"]["name"] == name)
                .unwrap_or_else(|| panic!("{name} should be advertised"));
            assert_eq!(
                spec["function"]["parameters"]["properties"]["placement"]["enum"],
                json!(["new_tab", "focused_tab", "floating"]),
                "{name} should take a closed set of placements",
            );
        }
    }

    /// `warp_pane_here` deliberately takes no `is_plugin`: a plugin pane has no
    /// process to move, and `list_other_sessions` never offers one.
    #[test]
    fn the_warp_schemas_require_a_destination_and_a_pane() {
        let specs = specs();
        for (name, required) in [
            ("warp_pane", json!(["pane_id", "is_plugin", "to_session"])),
            ("warp_pane_here", json!(["from_session", "pane_id"])),
            ("adopt_orphaned_pane", json!(["host_id"])),
            ("list_other_sessions", Value::Null),
            ("list_orphaned_panes", Value::Null),
        ] {
            let spec = specs
                .as_array()
                .unwrap()
                .iter()
                .find(|spec| spec["function"]["name"] == name)
                .unwrap_or_else(|| panic!("{name} should be advertised"));
            assert_eq!(
                spec["function"]["parameters"]["required"], required,
                "{name} requires the wrong arguments",
            );
        }
        let here = specs
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["function"]["name"] == "warp_pane_here")
            .unwrap();
        assert_eq!(
            here["function"]["parameters"]["properties"]["is_plugin"],
            Value::Null,
        );
    }

    #[test]
    fn an_absent_placement_lands_the_pane_in_a_tab_of_its_own() {
        assert_eq!(Args::default().placement(), Ok(WarpPlacement::NewTab));
    }

    #[test]
    fn every_advertised_placement_parses() {
        for (name, expected) in [
            ("new_tab", WarpPlacement::NewTab),
            ("focused_tab", WarpPlacement::FocusedTab),
            ("floating", WarpPlacement::Floating),
        ] {
            let args = Args {
                placement: Some(name.to_owned()),
                ..Args::default()
            };
            assert_eq!(args.placement(), Ok(expected), "{name} should parse");
        }
    }

    #[test]
    fn an_invented_placement_is_answered_with_the_ones_that_exist() {
        let args = Args {
            placement: Some("float".to_owned()),
            ..Args::default()
        };
        let error = args.placement().expect_err("float is not a placement");
        for legal in ["new_tab", "focused_tab", "floating"] {
            assert!(
                error.contains(legal),
                "the error should name {legal}, got: {error}"
            );
        }
    }

    /// `to` belongs to the Slack tools; a warp that quietly read it would send
    /// the pane to a channel name.
    #[test]
    fn the_warp_arguments_do_not_collide_with_the_slack_ones() {
        let args = Args::parse(r##"{"to":"#general","to_session":"other"}"##).unwrap();
        assert_eq!(args.to, Some("#general".to_owned()));
        assert_eq!(args.to_session, Some("other".to_owned()));
    }

    #[test]
    fn empty_arguments_parse_as_defaults() {
        assert!(Args::parse("").unwrap().pane_id.is_none());
        assert!(Args::parse("{}").unwrap().pane_id.is_none());
    }

    #[test]
    fn unknown_argument_keys_are_ignored_rather_than_failing_the_call() {
        let args = Args::parse(r#"{"pane_id":3,"invented_by_the_model":true}"#).unwrap();
        assert_eq!(args.pane_id, Some(3));
    }

    #[test]
    fn malformed_arguments_report_an_error_instead_of_panicking() {
        assert!(Args::parse("{not json").is_err());
    }

    #[test]
    fn a_string_command_absorbs_the_args_array() {
        let args = Args::parse(r#"{"command":"git","args":["status","--short"]}"#).unwrap();
        assert_eq!(args.command_words(), ["git", "status", "--short"]);
    }

    #[test]
    fn an_array_command_is_taken_whole() {
        let args = Args::parse(r#"{"command":["git","status"]}"#).unwrap();
        assert_eq!(args.command_words(), ["git", "status"]);
    }

    #[test]
    fn a_missing_command_yields_no_words() {
        assert!(Args::parse(r#"{"floating":true}"#)
            .unwrap()
            .command_words()
            .is_empty());
    }

    #[test]
    fn multi_pane_arguments_require_complete_pane_pairs() {
        let error = Args::parse(r#"{"panes":[{"pane_id":3}]}"#).unwrap_err();
        assert!(error.contains("is_plugin"), "{error}");
    }

    #[test]
    fn moving_panes_requires_exactly_one_destination() {
        let neither = Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}]}"#).unwrap();
        assert!(neither.move_destination().is_err());

        let both = Args::parse(
            r#"{"panes":[{"pane_id":3,"is_plugin":false}],"tab_index":1,"new_tab":true}"#,
        )
        .unwrap();
        assert!(both.move_destination().is_err());

        let existing =
            Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}],"tab_index":1}"#).unwrap();
        assert_eq!(
            existing.move_destination().unwrap(),
            MoveDestination::Existing(1)
        );

        let new = Args::parse(
            r#"{"panes":[{"pane_id":3,"is_plugin":false}],"new_tab":true,"new_tab_name":"logs"}"#,
        )
        .unwrap();
        assert_eq!(
            new.move_destination().unwrap(),
            MoveDestination::New(Some("logs".into()))
        );

        let by_id =
            Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}],"tab_id":47}"#).unwrap();
        assert_eq!(
            by_id.move_destination().unwrap(),
            MoveDestination::ExistingId(47)
        );

        let both_ways =
            Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}],"tab_index":1,"tab_id":47}"#)
                .unwrap();
        assert!(both_ways.move_destination().is_err());

        let id_and_new = Args::parse(
            r#"{"panes":[{"pane_id":3,"is_plugin":false}],"tab_id":47,"new_tab":true}"#,
        )
        .unwrap();
        assert!(id_and_new.move_destination().is_err());
    }

    #[test]
    fn a_tab_target_needs_exactly_one_of_index_or_id() {
        assert_eq!(
            Args::parse(r#"{"tab_index":2}"#)
                .unwrap()
                .tab_target()
                .unwrap(),
            TabTarget::Index(2)
        );
        assert_eq!(
            Args::parse(r#"{"tab_id":47}"#)
                .unwrap()
                .tab_target()
                .unwrap(),
            TabTarget::Id(47)
        );
        assert!(Args::parse("{}").unwrap().tab_target().is_err());
        assert!(Args::parse(r#"{"tab_index":2,"tab_id":47}"#)
            .unwrap()
            .tab_target()
            .is_err());
    }

    #[test]
    fn naming_no_tab_leaves_focus_and_rename_on_the_pane_path() {
        assert!(Args::parse(r#"{"pane_id":3,"is_plugin":false}"#)
            .unwrap()
            .tab_or_pane_target()
            .is_none());

        let both = Args::parse(r#"{"pane_id":3,"is_plugin":false,"tab_index":2}"#).unwrap();
        assert_eq!(
            both.tab_or_pane_target().unwrap().unwrap_err(),
            "give exactly one of pane pair, tab_index, or tab_id"
        );

        // A half-supplied pane pair still counts as naming a pane.
        let half = Args::parse(r#"{"is_plugin":false,"tab_id":47}"#).unwrap();
        assert!(half.tab_or_pane_target().unwrap().is_err());

        assert_eq!(
            Args::parse(r#"{"tab_id":47}"#)
                .unwrap()
                .tab_or_pane_target()
                .unwrap()
                .unwrap(),
            TabTarget::Id(47)
        );
    }

    #[test]
    fn stacking_requires_two_panes() {
        let args = Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}]}"#).unwrap();
        assert_eq!(
            args.pane_ids(2).unwrap_err(),
            "at least 2 panes are required"
        );
    }

    #[test]
    fn swapping_requires_exactly_two_panes() {
        let one = Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}]}"#).unwrap();
        assert_eq!(
            one.swap_pane_ids().unwrap_err(),
            "swap_panes needs exactly two panes"
        );

        let two = Args::parse(
            r#"{"panes":[{"pane_id":3,"is_plugin":false},{"pane_id":4,"is_plugin":true}]}"#,
        )
        .unwrap();
        assert_eq!(
            two.swap_pane_ids().unwrap(),
            [PaneId::Terminal(3), PaneId::Plugin(4)]
        );

        let three = Args::parse(
            r#"{"panes":[{"pane_id":3,"is_plugin":false},{"pane_id":4,"is_plugin":false},{"pane_id":5,"is_plugin":false}]}"#,
        )
        .unwrap();
        assert_eq!(
            three.swap_pane_ids().unwrap_err(),
            "swap_panes needs exactly two panes"
        );
    }

    #[test]
    fn floating_geometry_accepts_cells_and_percentages() {
        let args = Args::parse(r#"{"x":"2","y":"10%","width":"50%","height":"12"}"#).unwrap();
        let geometry = args.floating_geometry().unwrap();
        assert!(geometry.x.is_some());
        assert!(geometry.y.is_some());
        assert!(geometry.width.is_some());
        assert!(geometry.height.is_some());
    }

    #[test]
    fn floating_geometry_rejects_empty_invalid_and_zero_sizes() {
        assert!(Args::parse("{}").unwrap().floating_geometry().is_err());
        assert!(Args::parse(r#"{"x":"101%"}"#)
            .unwrap()
            .floating_geometry()
            .is_err());
        assert!(Args::parse(r#"{"width":"0"}"#)
            .unwrap()
            .floating_geometry()
            .is_err());
    }

    #[test]
    fn rerun_rejects_plugin_panes() {
        let args = Args::parse(r#"{"pane_id":3,"is_plugin":true}"#).unwrap();
        assert_eq!(
            args.terminal_pane_id().unwrap_err(),
            "rerun_pane only supports terminal command panes"
        );
    }

    fn positioned_pane(id: u32, x: usize, y: usize, columns: usize, rows: usize) -> PanePosition {
        PanePosition {
            id: PaneId::Terminal(id),
            tab_index: 0,
            x,
            y,
            columns,
            rows,
            visible: true,
            selectable: true,
            floating: false,
            fullscreen: false,
            floating_layer_visible: false,
        }
    }

    #[test]
    fn swap_planner_maps_each_unique_shared_edge_to_its_direction() {
        let source = positioned_pane(1, 10, 10, 10, 10);
        for (target, expected) in [
            (positioned_pane(2, 0, 10, 10, 10), PaneDirection::Left),
            (positioned_pane(2, 20, 10, 10, 10), PaneDirection::Right),
            (positioned_pane(2, 10, 0, 10, 10), PaneDirection::Up),
            (positioned_pane(2, 10, 20, 10, 10), PaneDirection::Down),
        ] {
            assert_eq!(
                plan_swap(&[source.clone(), target], source.id, PaneId::Terminal(2)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn swap_planner_rejects_panes_that_cannot_be_safely_swapped() {
        let source = positioned_pane(1, 0, 0, 10, 10);
        let target = positioned_pane(2, 10, 0, 10, 10);
        assert_eq!(
            plan_swap(&[source.clone(), target.clone()], source.id, source.id).unwrap_err(),
            "swap_panes needs two different panes"
        );

        let mut other_tab = target.clone();
        other_tab.tab_index = 1;
        assert_eq!(
            plan_swap(&[source.clone(), other_tab], source.id, target.id).unwrap_err(),
            "swap_panes needs both panes in the same tab"
        );

        let mut floating = target.clone();
        floating.floating = true;
        assert_eq!(
            plan_swap(&[source.clone(), floating], source.id, target.id).unwrap_err(),
            "swap_panes only supports visible selectable tiled panes"
        );

        let mut hidden = target.clone();
        hidden.visible = false;
        assert_eq!(
            plan_swap(&[source.clone(), hidden], source.id, target.id).unwrap_err(),
            "swap_panes only supports visible selectable tiled panes"
        );

        let mut unselectable = target.clone();
        unselectable.selectable = false;
        assert_eq!(
            plan_swap(&[source.clone(), unselectable], source.id, target.id).unwrap_err(),
            "swap_panes only supports visible selectable tiled panes"
        );

        assert_eq!(
            plan_swap(&[source.clone()], source.id, target.id).unwrap_err(),
            "pane is not in the current session; call list_session again"
        );

        let distant = positioned_pane(2, 11, 0, 10, 10);
        assert_eq!(
            plan_swap(&[source.clone(), distant], source.id, target.id).unwrap_err(),
            "panes are not adjacent; do not retry with move_pane"
        );
    }

    #[test]
    fn swap_planner_rejects_an_edge_with_multiple_possible_neighbors() {
        let source = positioned_pane(1, 0, 0, 10, 10);
        let upper = positioned_pane(2, 10, 0, 10, 5);
        let lower = positioned_pane(3, 10, 5, 10, 5);
        assert_eq!(
            plan_swap(&[source.clone(), upper.clone(), lower], source.id, upper.id).unwrap_err(),
            "the requested edge has multiple adjacent panes; swap is ambiguous"
        );

        let left = positioned_pane(1, 0, 0, 10, 5);
        let right = positioned_pane(2, 10, 0, 10, 10);
        let lower_left = positioned_pane(3, 0, 5, 10, 5);
        assert_eq!(
            plan_swap(
                &[left.clone(), right.clone(), lower_left],
                left.id,
                right.id
            )
            .unwrap_err(),
            "the requested edge has multiple adjacent panes; swap is ambiguous"
        );
    }

    #[test]
    fn planners_reject_tabs_where_zellij_will_not_move_tiled_panes() {
        let source = positioned_pane(1, 0, 0, 10, 10);
        let target = positioned_pane(2, 10, 0, 10, 10);

        let mut fullscreen = source.clone();
        fullscreen.fullscreen = true;
        assert!(plan_move(
            &[fullscreen, target.clone()],
            source.id,
            PaneDirection::Right
        )
        .is_err());

        let mut floating_layer = source.clone();
        floating_layer.floating_layer_visible = true;
        assert!(plan_swap(&[floating_layer, target], source.id, PaneId::Terminal(2)).is_err());
    }

    #[test]
    fn move_planner_requires_one_unique_neighbor_in_the_requested_direction() {
        let source = positioned_pane(1, 0, 0, 10, 10);
        let target = positioned_pane(2, 10, 0, 10, 10);
        assert_eq!(
            plan_move(
                &[source.clone(), target.clone()],
                source.id,
                PaneDirection::Right
            )
            .unwrap(),
            target.id
        );

        assert_eq!(
            plan_move(&[source.clone()], source.id, PaneDirection::Right).unwrap_err(),
            "there is no adjacent pane in that direction"
        );

        let lower = positioned_pane(3, 10, 5, 10, 5);
        assert_eq!(
            plan_move(
                &[source.clone(), target, lower],
                source.id,
                PaneDirection::Right
            )
            .unwrap_err(),
            "the requested direction has multiple adjacent panes; move is ambiguous"
        );
    }

    #[test]
    fn pane_direction_rejects_unknown_names() {
        assert_eq!(
            PaneDirection::parse(Some("diagonal")).unwrap_err(),
            "direction must be left, right, up or down"
        );
    }

    #[test]
    fn send_input_schema_requires_a_pane_and_advertises_the_closed_key_set() {
        let specs = specs();
        let send_input = specs
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["function"]["name"] == "send_input")
            .unwrap();
        assert_eq!(
            send_input["function"]["parameters"]["required"],
            json!(["pane_id", "is_plugin"])
        );
        assert_eq!(
            send_input["function"]["parameters"]["properties"]["keys"]["items"]["enum"],
            json!([
                "enter",
                "tab",
                "escape",
                "backspace",
                "delete",
                "insert",
                "up",
                "down",
                "left",
                "right",
                "home",
                "end",
                "page_up",
                "page_down",
                "ctrl_a",
                "ctrl_b",
                "ctrl_c",
                "ctrl_d",
                "ctrl_e",
                "ctrl_f",
                "ctrl_k",
                "ctrl_l",
                "ctrl_u",
                "ctrl_w",
                "ctrl_y",
                "ctrl_z"
            ])
        );
    }

    #[test]
    fn send_input_encodes_text_before_keys() {
        let args = Args::parse(r#"{"text":"hé","keys":["enter"]}"#).unwrap();
        assert_eq!(
            args.input_chunks(false).unwrap(),
            vec![b"h\xc3\xa9".to_vec(), b"\r".to_vec()]
        );
    }

    #[test]
    fn send_input_brackets_only_the_text_and_only_when_asked() {
        let args = Args::parse(r#"{"text":"hé","keys":["enter"]}"#).unwrap();
        assert_eq!(
            args.input_chunks(true).unwrap(),
            vec![
                b"\x1b[200~".to_vec(),
                b"h\xc3\xa9".to_vec(),
                b"\x1b[201~".to_vec(),
                b"\r".to_vec(),
            ]
        );
        // No text, no markers: a lone key must stay a lone key so Zellij's own
        // key adjustment still matches the whole buffer.
        let keys_only = Args::parse(r#"{"keys":["up"]}"#).unwrap();
        assert_eq!(
            keys_only.input_chunks(true).unwrap(),
            vec![b"\x1b[A".to_vec()]
        );
    }

    #[test]
    fn send_input_encodes_every_named_key() {
        let args = Args::parse(
            r#"{"keys":["enter","tab","escape","backspace","delete","insert","up","down","left","right","home","end","page_up","page_down","ctrl_a","ctrl_b","ctrl_c","ctrl_d","ctrl_e","ctrl_f","ctrl_k","ctrl_l","ctrl_u","ctrl_w","ctrl_y","ctrl_z"]}"#,
        )
        .unwrap();
        assert_eq!(
            args.input_chunks(false).unwrap(),
            vec![
                vec![13],
                vec![9],
                vec![27],
                vec![127],
                vec![27, 91, 51, 126],
                vec![27, 91, 50, 126],
                vec![27, 91, 65],
                vec![27, 91, 66],
                vec![27, 91, 68],
                vec![27, 91, 67],
                vec![27, 91, 72],
                vec![27, 91, 70],
                vec![27, 91, 53, 126],
                vec![27, 91, 54, 126],
                vec![1],
                vec![2],
                vec![3],
                vec![4],
                vec![5],
                vec![6],
                vec![11],
                vec![12],
                vec![21],
                vec![23],
                vec![25],
                vec![26],
            ]
        );
    }

    #[test]
    fn send_input_rejects_empty_and_unknown_input() {
        assert_eq!(
            Args::parse("{}").unwrap().input_chunks(true).unwrap_err(),
            "send_input needs non-empty text or at least one key"
        );
        let unknown = Args::parse(r#"{"keys":["f13"]}"#).unwrap();
        assert_eq!(
            unknown.input_chunks(true).unwrap_err(),
            "unsupported key: f13"
        );
    }

    #[test]
    fn tab_move_result_does_not_mislabel_zellij_stable_tab_id_as_an_index() {
        assert_eq!(
            moved_panes_result(Some(47)),
            "moved; call list_session for the tab's current index"
        );
        assert_eq!(moved_panes_result(None), "zellij did not move the panes");
    }

    #[test]
    fn summarize_omits_an_empty_argument_object() {
        let call = ToolCall {
            id: "1".into(),
            name: "list_session".into(),
            arguments: "{}".into(),
        };
        assert_eq!(summarize(&call), "list_session");
    }

    #[test]
    fn summarize_shows_the_arguments_verbatim() {
        let call = ToolCall {
            id: "1".into(),
            name: "close_pane".into(),
            arguments: r#"{"pane_id":3,"is_plugin":false}"#.into(),
        };
        assert_eq!(
            summarize(&call),
            r#"close_pane {"pane_id":3,"is_plugin":false}"#
        );
    }

    #[test]
    fn a_bare_command_takes_no_arguments() {
        assert_eq!(
            parse_command("/list_session"),
            Ok(("list_session".to_owned(), "{}".to_owned()))
        );
    }

    /// The point of the pairs: values must come out typed, under the names `Args`
    /// actually deserializes.
    #[test]
    fn pairs_are_typed_and_land_on_the_right_args_fields() {
        let (name, arguments) = parse_command("/read_pane pane_id=3 is_plugin=false").unwrap();
        assert_eq!(name, "read_pane");
        let args = Args::parse(&arguments).unwrap();
        assert_eq!(args.pane_id, Some(3));
        assert_eq!(args.is_plugin, Some(false));

        let (_, arguments) = parse_command("/new_pane name=logs floating=true").unwrap();
        let args = Args::parse(&arguments).unwrap();
        assert_eq!(args.name.as_deref(), Some("logs"));
        assert!(args.floating);

        let json = r#"/stack_panes panes=[{"pane_id":1,"is_plugin":false}]"#;
        let (_, arguments) = parse_command(json).unwrap();
        assert_eq!(Args::parse(&arguments).unwrap().panes.len(), 1);
    }

    #[test]
    fn a_json_rest_is_passed_through_verbatim() {
        assert_eq!(
            parse_command(r#"/rename {"name": "my tab"}"#),
            Ok(("rename".to_owned(), r#"{"name": "my tab"}"#.to_owned()))
        );
    }

    #[test]
    fn a_token_without_an_equals_is_rejected() {
        assert!(parse_command("/read_pane 3").is_err());
        assert!(parse_command("/").is_err());
    }

    #[test]
    fn names_lists_the_advertised_tools() {
        let names = names();
        assert_eq!(names.len(), specs().as_array().unwrap().len());
        assert!(names.contains(&"new_pane".to_owned()));
    }

    /// The one argument each todo tool cannot work without. `todo_list` needs
    /// none: it falls back to the active project.
    #[test]
    fn the_todo_tools_require_their_key_argument() {
        let specs = specs();
        let required = |name: &str| -> Vec<String> {
            specs
                .as_array()
                .unwrap()
                .iter()
                .find(|spec| spec["function"]["name"] == name)
                .map(|spec| {
                    spec["function"]["parameters"]["required"]
                        .as_array()
                        .map(|names| {
                            names
                                .iter()
                                .map(|name| name.as_str().unwrap().to_owned())
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .expect("the tool is advertised")
        };
        assert_eq!(required("todo_add"), vec!["text".to_owned()]);
        assert_eq!(required("todo_search"), vec!["query".to_owned()]);
        assert_eq!(required("todo_done"), vec!["id".to_owned()]);
        assert!(required("todo_list").is_empty());
    }

    #[test]
    fn args_parse_reads_the_todo_fields() {
        let args = Args::parse(
            r#"{"id":7,"parent_id":3,"include_done":true,"all_projects":true,"undo":true}"#,
        )
        .unwrap();

        assert_eq!(args.id, Some(7));
        assert_eq!(args.parent_id, Some(3));
        assert!(args.include_done);
        assert!(args.all_projects);
        assert!(args.undo);
    }
}

//! The tool set the model uses to drive Zellij.
//!
//! Split in two halves. Everything above the dispatch section is pure and
//! host-testable; `dispatch` and its helpers call the Zellij host and are not.
//! That split is load-bearing rather than stylistic: `host_run_plugin_command`
//! is a no-op on non-wasm32 targets, and most value-returning shims `.unwrap()`
//! the protobuf decode, so calling one from a host-triple test panics.

use hocus_proto::paths;
use hocus_proto::projects::{self, Project};
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
                    "tab_index": { "type": "integer" },
                    "tab_id": { "type": "integer", "description": "Prefer over tab_index." },
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
                    "tab_index": { "type": "integer" },
                    "tab_id": { "type": "integer" },
                    "name": { "type": "string" },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "move_pane",
            "Move one tiled pane toward a unique neighbor; use swap_panes for two named panes and set_floating_pane_geometry for floating panes.",
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
                    "tab_index": { "type": "integer" },
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
                    "tab_index": { "type": "integer" },
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
            "schedule",
            "Repeat a prompt until cancelled; unschedule it when done.",
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
            "Add a persistent project todo, optionally under a parent.",
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Up to 500 characters." },
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
            "Open and register a tiled agent pane.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "cwd": { "type": "string", "description": "Defaults to the focused pane's." },
                    "project": { "type": "string", "description": "Defaults to the active project." },
                    "kind": {
                        "type": "string",
                        "description": "For unrecognized agent commands.",
                    },
                },
                "required": ["command"],
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
            "Regex-search git-visible text files inside the active project.",
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
                    "start_line": { "type": "integer", "description": "1-based; defaults to 1." },
                    "line_count": { "type": "integer", "description": "Defaults to 300." },
                },
                "required": ["path"],
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
    // The file tools. `query` above is `search_files`'s pattern.
    pub path: Option<String>,
    pub pattern: Option<String>,
    pub glob: Option<String>,
    pub case_insensitive: bool,
    pub max_results: Option<usize>,
    pub start_line: Option<usize>,
    pub line_count: Option<usize>,
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

/// The tools that read the project's own files. Intercepted in `run_head` for
/// two reasons: they need the active project, which `dispatch` cannot reach, and
/// the plugin cannot read those files itself.
///
/// A wasm plugin sees only its WASI preopens — `/host`, `/data`, `/cache`,
/// `/tmp` — and a project root is none of those, so `std::fs` cannot open a file
/// in the user's checkout at all. These shell out the way the Slack tools do.
pub const FILE_TOOLS: [&str; 3] = ["find_files", "search_files", "read_file"];

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
    if !projects::contains(project, &resolved) {
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
            // Reading the project's own files changes nothing, and the root
            // check is what keeps them the project's own.
            "find_files",
            "search_files",
            "read_file",
        ] {
            assert!(!needs_confirmation(name), "{name} should not be confirmed");
        }
    }

    fn argv(name: &str, arguments: &str) -> Result<Vec<String>, String> {
        slack_argv(name, &Args::parse(arguments).unwrap())
    }

    fn project() -> Project {
        Project {
            name: "hocus".to_owned(),
            root: "/work/hocus".to_owned(),
            remote: None,
            worktree_status: None,
            common_dir: None,
            discovered: false,
        }
    }

    fn file_argv_for(name: &str, arguments: &str) -> Result<Vec<String>, String> {
        let args = Args::parse(arguments).unwrap();
        let target = resolve_in_root(&project(), args.path.as_deref())?;
        file_argv(name, &args, &target)
    }

    #[test]
    fn a_relative_path_is_taken_from_the_project_root() {
        assert_eq!(
            resolve_in_root(&project(), Some("src/main.rs")).unwrap(),
            "/work/hocus/src/main.rs"
        );
        // The whole project when the model names no path at all.
        assert_eq!(resolve_in_root(&project(), None).unwrap(), "/work/hocus");
        assert_eq!(
            resolve_in_root(&project(), Some("  ")).unwrap(),
            "/work/hocus"
        );
    }

    /// An absolute path is what `find_files` and `search_files` hand back, so
    /// feeding one straight to `read_file` has to work.
    #[test]
    fn an_absolute_path_inside_the_root_is_kept() {
        assert_eq!(
            resolve_in_root(&project(), Some("/work/hocus/src/main.rs")).unwrap(),
            "/work/hocus/src/main.rs"
        );
    }

    #[test]
    fn a_path_that_climbs_out_of_the_project_is_refused() {
        assert_eq!(
            resolve_in_root(&project(), Some("../secrets")),
            Err("path must not contain a .. component".to_owned())
        );
        // Rejected even though it lands back inside: resolving it needs the
        // filesystem, and refusing is the answer that needs no filesystem.
        assert!(resolve_in_root(&project(), Some("src/../src/main.rs")).is_err());
    }

    #[test]
    fn an_absolute_path_outside_the_project_is_refused() {
        assert_eq!(
            resolve_in_root(&project(), Some("/etc/passwd")),
            Err("path must be inside project hocus".to_owned())
        );
    }

    /// The trap a plain string prefix falls into: `/work/hocus-evil` starts with
    /// `/work/hocus` as text but is a different folder.
    #[test]
    fn a_sibling_whose_name_extends_the_root_is_outside_it() {
        assert!(resolve_in_root(&project(), Some("/work/hocus-evil/x")).is_err());
    }

    #[test]
    fn find_files_wraps_a_bare_name_and_leaves_a_glob_alone() {
        assert_eq!(
            file_argv_for("find_files", r#"{"pattern": "tool_loop"}"#).unwrap(),
            [
                "rg",
                "--files",
                "--glob",
                "*tool_loop*",
                "--",
                "/work/hocus"
            ]
        );
        assert_eq!(
            file_argv_for("find_files", r#"{"pattern": "*.rs", "path": "src"}"#).unwrap(),
            ["rg", "--files", "--glob", "*.rs", "--", "/work/hocus/src"]
        );
        // A pattern with a separator is a path shape the model meant literally.
        assert_eq!(glob_for("src/**/main.rs"), "src/**/main.rs");
    }

    #[test]
    fn search_files_passes_the_query_after_a_flag_of_its_own() {
        assert_eq!(
            file_argv_for("search_files", r#"{"query": "fn main"}"#).unwrap(),
            [
                "rg",
                "--line-number",
                "--with-filename",
                "--color",
                "never",
                "-e",
                "fn main",
                "--",
                "/work/hocus"
            ]
        );
        // A query that looks like an option is still a pattern, because `-e`
        // consumes the next argument whatever it says.
        let argv = file_argv_for("search_files", r#"{"query": "--help"}"#).unwrap();
        assert_eq!(argv[argv.len() - 3..], ["--help", "--", "/work/hocus"]);
    }

    #[test]
    fn search_files_adds_only_the_options_it_was_given() {
        let argv = file_argv_for(
            "search_files",
            r#"{"query": "x", "case_insensitive": true, "glob": "*.rs"}"#,
        )
        .unwrap();
        assert!(argv.contains(&"--ignore-case".to_owned()));
        assert_eq!(argv.iter().filter(|a| *a == "--glob").count(), 1);
        assert!(!file_argv_for("search_files", r#"{"query": "x"}"#)
            .unwrap()
            .contains(&"--ignore-case".to_owned()));
    }

    #[test]
    fn read_file_asks_for_the_window_it_was_given() {
        assert_eq!(
            file_argv_for(
                "read_file",
                r#"{"path": "src/main.rs", "start_line": 10, "line_count": 5}"#
            )
            .unwrap(),
            ["sed", "-n", "10,14p", "--", "/work/hocus/src/main.rs"]
        );
    }

    /// An unbounded read would be cut mid-line by MAX_TOOL_OUTPUT after the host
    /// had already read the whole file, so the default window is a real one.
    #[test]
    fn read_file_bounds_a_request_that_names_no_window() {
        assert_eq!(
            file_argv_for("read_file", r#"{"path": "x"}"#).unwrap()[2],
            format!("1,{FILE_TOOL_LINES}p")
        );
    }

    #[test]
    fn file_tools_reject_a_missing_required_argument() {
        assert!(file_argv_for("find_files", "{}").is_err());
        assert!(file_argv_for("search_files", r#"{"query": "  "}"#).is_err());
        assert!(file_argv_for("read_file", "{}").is_err());
    }

    /// `read_file` returns what was asked for; a listing is capped so a search of
    /// a large repository does not fill the transcript.
    #[test]
    fn only_listings_are_capped() {
        let args = Args::parse(r#"{"max_results": 5}"#).unwrap();
        assert_eq!(file_line_cap("read_file", &args), None);
        assert_eq!(file_line_cap("search_files", &args), Some(5));
        assert_eq!(
            file_line_cap("find_files", &Args::default()),
            Some(FILE_TOOL_LINES)
        );
        // Asking for everything still stops somewhere.
        let huge = Args::parse(r#"{"max_results": 99999}"#).unwrap();
        assert_eq!(
            file_line_cap("find_files", &huge),
            Some(MAX_FILE_TOOL_LINES)
        );
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
        assert_eq!(names.len(), 49);
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
    fn hocus_prompt_contract_keeps_tool_specific_routing_cues() {
        let specs = specs();
        for (name, cues) in [
            (
                "move_pane",
                &["swap_panes", "set_floating_pane_geometry"][..],
            ),
            ("warp_pane_here", &["list_other_sessions"][..]),
            ("schedule", &["unschedule"][..]),
            ("slack_read", &["backfill"][..]),
            ("slack_post", &["immediately"][..]),
            ("agent_send", &["listed agent"][..]),
            ("find_files", &["active project"][..]),
            ("search_files", &["active project"][..]),
            ("read_file", &["active project"][..]),
            ("close_pane", &["process"][..]),
        ] {
            let description = specs
                .as_array()
                .unwrap()
                .iter()
                .find(|spec| spec["function"]["name"] == name)
                .unwrap_or_else(|| panic!("{name} is not advertised"))["function"]["description"]
                .as_str()
                .unwrap();
            for cue in cues {
                assert!(description.contains(cue), "{name} is missing {cue:?}");
            }
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

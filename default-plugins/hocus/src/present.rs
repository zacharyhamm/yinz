//! Turning a tool result into something a person reads.
//!
//! A tool result is written for the model: `list_session` answers with one long
//! line of compact JSON, `clear_pane` with the bare word `cleared`. That is the
//! right answer for the transcript and the wrong one for the pane when the
//! *user* typed the command — which they can, for every advertised tool (see
//! `tools::parse_command`).
//!
//! So this module renders the same result a second time, for the person. The
//! model is unaffected: what it receives is `Message::content`, untouched, and
//! this only ever fills `Message::display`.
//!
//! ## Why markdown
//!
//! `ui::markdown_lines` already turns markdown into styled lines — headings,
//! nested lists, bold, code blocks — so emitting markdown buys all of that
//! without a second renderer. Two limits of it shape everything below:
//!
//! - **No tables.** The parser runs with `Options::empty()` and there is no
//!   `Tag::Table` arm, so pipe rows would word-wrap into mush. Anything that
//!   wants columns goes in a fenced block, via [`table`].
//! - **Untrusted text restyles the transcript.** Pane titles, branch names and
//!   Slack messages all reach here, and a title containing `#` or `*` would be
//!   read as markup. Everything interpolated goes through [`fence`] or
//!   [`inline`].
//!
//! ## Why `Option`
//!
//! `None` means "show the raw string exactly as it arrived", and it is the
//! common answer, not the failure case. Errors come back as ordinary result
//! strings — `no such tool: x`, `pane_id and is_plugin are both required` — and
//! they are already the clearest thing to show. Falling back is also what keeps
//! a tool added later from being made *worse* than it is today.

use crate::tools::{self, ToolCall};
use serde_json::Value;

/// The markdown a raw tool result should be shown as, or `None` to show the raw
/// string exactly as it arrived.
pub fn present(call: &ToolCall, result: &str) -> Option<String> {
    let args = serde_json::from_str::<Value>(&call.arguments).unwrap_or(Value::Null);
    let name = call.name.as_str();

    // `agent_spawn` answers with `new_pane`'s JSON and a note glued on the end
    // by `State::register_spawned`, which runs before `finish_head`. Split here
    // rather than in its own arm so what remains parses like any other result.
    let (result, registered) = match name {
        "agent_spawn" => match result.split_once(" (registered as ") {
            Some((json, rest)) => (json, rest.strip_suffix(')')),
            None => (result, None),
        },
        _ => (result, None),
    };

    // The only test for "this is machine output" that does not guess: no error
    // sentence any tool returns is valid JSON.
    let json = serde_json::from_str::<Value>(result)
        .ok()
        .filter(|value| value.is_object() || value.is_array());

    // Tools whose own answer is the best answer, whatever it looks like. These
    // return early rather than falling through, because the generic renderer
    // would happily "improve" prose into a bullet list.
    if PROSE.contains(&name) {
        return None;
    }
    if tools::SLACK_TOOLS.contains(&name) {
        return slack(name, result);
    }
    if tools::FILE_TOOLS.contains(&name) {
        return file_tool(name, &args, result);
    }
    if let Some(sentence) = ack(name, result, &args) {
        return Some(sentence);
    }

    // Everything below is structured, or is an error string that isn't and so
    // stops here.
    let json = json?;
    let shaped = match name {
        "list_session" => session(&json),
        "list_other_sessions" => other_sessions(&json),
        "list_orphaned_panes" => orphans(&json),
        "project_list" => projects(&json),
        "agent_list" => agents(&json),
        "todo_list" => todo_list(&json),
        "todo_search" => todo_search(&json),
        "new_pane" => opened_pane(&json),
        "new_tab" => opened_tab(&json),
        "todo_add" | "todo_edit" | "todo_done" => todo_written(name, &json),
        "agent_spawn" => spawned(&json, registered),
        _ => None,
    };
    // A tool's own arm declining is not a reason to fall back to raw JSON: the
    // generic renderer still beats braces, and it is what covers every tool
    // added after this was written.
    shaped.or_else(|| generic(&json))
}

/// Tools that answer in prose by design. Presenting one would mean rewriting a
/// sentence that already says what happened, in the terms the user asked in.
const PROSE: &[&str] = &[
    "schedule",
    "unschedule",
    "project_assign",
    "project_forget",
    "project_open",
    "agent_register",
    "web_search",
    // Raw scrollback. Fencing it would be a lie about what it is, and the plain
    // path already renders it whole.
    "read_pane",
];

// ---------------------------------------------------------------------------
// The command list
// ---------------------------------------------------------------------------

/// The families `/help` groups the commands into, in the order it lists them.
///
/// Matched on the words of a tool's name rather than on an exhaustive list, so
/// a tool added later lands in the right group without this being touched.
/// Anything matching nothing falls into a trailing "other" group, which is what
/// keeps the list complete without keeping it in sync by hand.
///
/// Order matters: the first family to claim a name keeps it. That is what puts
/// `warp_tab` under tabs rather than letting the pane family's `warp` take it,
/// so the narrow families come before the broad ones.
const FAMILIES: &[(&str, &[&str])] = &[
    ("session", &["list_session", "list_other_sessions"]),
    (
        "tabs",
        &["new_tab", "close_tab", "warp_tab", "warp_tab_here"],
    ),
    (
        "panes",
        &[
            "pane",
            "panes",
            "focus",
            "rename",
            "toggle",
            "stack",
            "swap",
            "scroll",
            "warp",
            "send_input",
        ],
    ),
    ("agents", &["agent"]),
    ("projects", &["project"]),
    ("todos", &["todo"]),
    ("files", &["find_files", "search_files", "read_file"]),
    ("slack", &["slack"]),
    ("chat", &["schedule", "unschedule", "web_search"]),
];

/// Every command that can be typed, grouped so the list can be read.
///
/// The old `/help` was all fifty-odd names in one unbroken run of text, which
/// is a list you can only search, not scan.
///
/// Plain text, not markdown: this goes out as a `Message::notice`, which the
/// transcript renders through `wrap` rather than through the markdown pass, so
/// any syntax here would be shown literally. The grouping is the whole benefit
/// and it survives being plain.
pub fn commands() -> String {
    let mut groups: Vec<(String, Vec<String>)> = vec![(
        "hocus".to_owned(),
        crate::LOCAL_COMMANDS
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
    )];

    let all = tools::names();
    let mut placed: Vec<String> = Vec::new();
    for (family, prefixes) in FAMILIES {
        let names: Vec<String> = all
            .iter()
            .filter(|name| !placed.contains(name))
            .filter(|name| prefixes.iter().any(|prefix| in_family(name, prefix)))
            .cloned()
            .collect();
        if names.is_empty() {
            continue;
        }
        placed.extend(names.iter().cloned());
        groups.push(((*family).to_owned(), names));
    }
    let rest: Vec<String> = all
        .iter()
        .filter(|name| !placed.contains(name))
        .cloned()
        .collect();
    if !rest.is_empty() {
        groups.push(("other".to_owned(), rest));
    }

    // The labels are padded into a column so the names line up under each
    // other, which is what makes the groups visible at a glance.
    let width = groups
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or_default();
    groups
        .iter()
        .map(|(label, names)| {
            format!(
                "{label:<width$}  {}",
                names
                    .iter()
                    .map(|name| format!("/{name}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a tool belongs to a family, by whole word rather than by substring:
/// `rename` is a pane command, `read_pane` is too, and `agent_send` must not be
/// dragged in by `send`.
fn in_family(name: &str, prefix: &str) -> bool {
    name == prefix || name.split('_').any(|word| word == prefix)
}

// ---------------------------------------------------------------------------
// Listings
// ---------------------------------------------------------------------------

/// Convenience for the listings: a field off a JSON object, as text.
fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn number(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|n| n.to_string())
        .unwrap_or_default()
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map_or(&[], |v| v)
}

/// The current session: one block per tab, each a table of its panes.
///
/// The ids come first because they are what every follow-up command needs —
/// this listing exists to be read and then typed back.
fn session(value: &Value) -> Option<String> {
    let mut out = heading(&format!("session {}", text(value, "session")));
    let tabs = array(value, "tabs");
    if tabs.is_empty() {
        return Some(out + "_no tabs_\n");
    }
    for tab in tabs {
        let name = text(tab, "name");
        let marker = if flag(tab, "active") {
            " — active"
        } else {
            ""
        };
        out.push_str(&format!(
            "**{}: {}**{marker}\n\n",
            number(tab, "tab_number"),
            inline(&name)
        ));
        out.push_str(&pane_table(array(tab, "panes")));
        out.push('\n');
    }
    out.push_str(&legend(tabs));
    Some(out)
}

/// What the marks column means, printed once and only when something in the
/// listing actually carries a mark. A key nobody needs is just noise, and a
/// column of bare punctuation with no key is unreadable.
fn legend(tabs: &[Value]) -> String {
    let panes = || tabs.iter().flat_map(|tab| array(tab, "panes"));
    let mut keys = Vec::new();
    for (key, mark, label) in [
        ("focused", "*", "focused"),
        ("floating", "~", "floating"),
        ("is_plugin", "p", "plugin"),
    ] {
        if panes().any(|pane| flag(pane, key)) {
            keys.push(format!("{mark} {label}"));
        }
    }
    if keys.is_empty() {
        return String::new();
    }
    format!("_{}_\n", keys.join(" · "))
}

/// The panes of one tab. Shared by both session listings, which report the same
/// fields with `list_other_sessions` leaving out the ones only meaningful here.
fn pane_table(panes: &[Value]) -> String {
    if panes.is_empty() {
        return "_no panes_\n".to_owned();
    }
    let rows: Vec<Vec<String>> = panes
        .iter()
        .map(|pane| {
            let mut marks = String::new();
            if flag(pane, "focused") {
                marks.push('*');
            }
            if flag(pane, "floating") {
                marks.push('~');
            }
            if flag(pane, "is_plugin") {
                marks.push('p');
            }
            vec![
                number(pane, "pane_id"),
                marks,
                text(pane, "title"),
                exit_note(pane),
            ]
        })
        .collect();
    table(&["id", "", "title", "status"], &rows)
}

/// What became of a pane's command, if it had one.
fn exit_note(pane: &Value) -> String {
    match pane.get("exit_status").and_then(Value::as_i64) {
        Some(0) => "exited".to_owned(),
        Some(code) => format!("exited {code}"),
        None => match pane.get("command") {
            Some(Value::String(command)) if !command.is_empty() => "running".to_owned(),
            _ => String::new(),
        },
    }
}

fn other_sessions(value: &Value) -> Option<String> {
    let mut out = heading("other sessions");
    for session in array(value, "sessions") {
        out.push_str(&format!("**{}**\n\n", inline(&text(session, "session"))));
        for tab in array(session, "tabs") {
            out.push_str(&format!(
                "{}: {}\n\n",
                number(tab, "tab_number"),
                inline(&text(tab, "name"))
            ));
            out.push_str(&pane_table(array(tab, "panes")));
            out.push('\n');
        }
    }
    Some(out)
}

/// Panes whose session went away. `host_id` rather than a pane id, because that
/// is what `adopt_orphaned_pane` takes.
fn orphans(value: &Value) -> Option<String> {
    let orphans = array(value, "orphans");
    if orphans.is_empty() {
        return Some(heading("orphaned panes") + "_none_\n");
    }
    let rows: Vec<Vec<String>> = orphans
        .iter()
        .map(|orphan| {
            vec![
                text(orphan, "host_id"),
                text(orphan, "command"),
                text(orphan, "session"),
                age(orphan.get("age_seconds").and_then(Value::as_u64)),
                if flag(orphan, "exited") {
                    "exited".to_owned()
                } else {
                    "running".to_owned()
                },
            ]
        })
        .collect();
    Some(heading("orphaned panes") + &table(&["host_id", "command", "was in", "age", ""], &rows))
}

/// A duration a person reads, not a second count. Minutes are the finest label
/// for the same reason `agents::status_cell` stops there: nothing here changes
/// meaningfully faster.
fn age(seconds: Option<u64>) -> String {
    match seconds {
        None => String::new(),
        Some(seconds) if seconds < 60 => "just now".to_owned(),
        Some(seconds) if seconds < 3600 => format!("{}m", seconds / 60),
        Some(seconds) if seconds < 86400 => format!("{}h", seconds / 3600),
        Some(seconds) => format!("{}d", seconds / 86400),
    }
}

fn projects(value: &Value) -> Option<String> {
    let active = text(value, "active");
    let projects = array(value, "projects");
    if projects.is_empty() {
        return Some(heading("projects") + "_none registered_\n");
    }
    let mut out = heading("projects");
    for project in projects {
        let name = text(project, "name");
        let marker = if !active.is_empty() && name == active {
            " — active"
        } else {
            ""
        };
        out.push_str(&format!("- **{}**{marker}\n", inline(&name)));
        out.push_str(&format!("  - {}\n", inline(&text(project, "root"))));
        let status = text(project, "worktree_status");
        if !status.is_empty() {
            out.push_str(&format!("  - {}\n", inline(&status)));
        }
        let tabs = array(project, "tab_ids");
        if !tabs.is_empty() {
            let ids: Vec<String> = tabs.iter().map(|id| id.to_string()).collect();
            out.push_str(&format!("  - tabs {}\n", ids.join(", ")));
        }
    }
    Some(out)
}

/// The agents, grouped the way the agent strip groups them — project, then
/// branch, then the agents themselves.
///
/// `agents::strip_rows` builds exactly this shape, but from the pane snapshot
/// and four registries; all we have here is the JSON `agents::list_json`
/// already flattened. So the grouping is rebuilt rather than reused.
fn agents(value: &Value) -> Option<String> {
    // `agents::list_json` answers with a bare array, not an object.
    let agents = value.as_array().map_or(&[][..], |items| items);
    if agents.is_empty() {
        return Some(heading("agents") + "_none_\n");
    }

    // Grouped rather than assumed grouped: `list_json` walks the pane snapshot
    // in pane order, so agents of one project can arrive interleaved. A
    // BTreeMap also puts the headers in a stable order between calls, which a
    // listing the user re-reads should have.
    let mut groups: std::collections::BTreeMap<(String, String), Vec<&Value>> = Default::default();
    for agent in agents {
        let project = or_else(text(agent, "project"), "no project");
        let branch = or_else(text(agent, "branch"), "no branch");
        groups.entry((project, branch)).or_default().push(agent);
    }

    let mut out = heading("agents");
    for ((project, branch), agents) in groups {
        out.push_str(&format!(
            "\n**{}** · {}\n\n",
            inline(&project),
            inline(&branch)
        ));
        for agent in agents {
            out.push_str(&format!(
                "- `{}` {} — {}\n",
                number(agent, "pane_id"),
                idle(agent),
                inline(&text(agent, "title"))
            ));
        }
    }
    Some(out)
}

/// `text` with a stand-in for the empty case, which every grouping header needs.
fn or_else(text: String, fallback: &str) -> String {
    if text.is_empty() {
        fallback.to_owned()
    } else {
        text
    }
}

/// An agent's status, with `idle_seconds` folded in the way the strip folds it
/// (`agents::status_cell`): running and never idle is busy, under a minute is
/// idle, beyond that it is worth saying how long.
fn idle(agent: &Value) -> String {
    let status = text(agent, "status");
    if status != "running" {
        return status;
    }
    match agent.get("idle_seconds").and_then(Value::as_u64) {
        None => "running".to_owned(),
        Some(0) => "busy".to_owned(),
        Some(seconds) if seconds < 60 => "idle".to_owned(),
        Some(seconds) => format!("idle {}m", seconds / 60),
    }
}

fn todo_list(value: &Value) -> Option<String> {
    let project = match text(value, "project") {
        name if name.is_empty() => text(value, "root"),
        name => name,
    };
    let mut out = heading(&format!("todos — {project}"));
    out.push_str(&format!(
        "{} open, {} done\n\n",
        number(value, "open"),
        number(value, "done")
    ));
    let items = array(value, "items");
    if items.is_empty() {
        return Some(out + "_nothing open_\n");
    }
    todo_items(items, 0, &mut out);
    Some(out)
}

/// The item tree as nested bullets, ids kept because every todo command takes
/// one.
fn todo_items(items: &[Value], depth: usize, out: &mut String) {
    for item in items {
        let box_ = if flag(item, "done") { "[x]" } else { "[ ]" };
        out.push_str(&format!(
            "{}- {box_} `{}` {}\n",
            "  ".repeat(depth),
            number(item, "id"),
            inline(&text(item, "text"))
        ));
        todo_items(array(item, "children"), depth + 1, out);
    }
}

fn todo_search(value: &Value) -> Option<String> {
    let matches = array(value, "matches");
    let mut out = heading(&format!("todo search — {}", text(value, "query")));
    if matches.is_empty() {
        return Some(out + "_no matches_\n");
    }
    for found in matches {
        let box_ = if flag(found, "done") { "[x]" } else { "[ ]" };
        let project = match text(found, "project") {
            name if name.is_empty() => text(found, "root"),
            name => name,
        };
        out.push_str(&format!(
            "- {box_} `{}` {} — _{}_\n",
            number(found, "id"),
            inline(&text(found, "text")),
            inline(&project)
        ));
    }
    if flag(value, "truncated") {
        out.push_str("\n_more matches were cut; narrow the query_\n");
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Acknowledgement verbs
// ---------------------------------------------------------------------------

/// `(tool, the exact result it returns on success, what to say instead)`.
///
/// Matching the result *exactly* is what separates a success from a failure
/// without reading the prose: every one of these tools answers with one of its
/// own error strings otherwise, and those fall through to `None` and are shown
/// as they are. `{pane}` is filled from the call's arguments.
const ACKS: &[(&str, &str, &str)] = &[
    ("clear_pane", "cleared", "Cleared {pane}'s scrollback."),
    ("close_pane", "closed", "Closed {pane}."),
    ("close_tab", "closed", "Closed the tab."),
    ("focus", "focused", "Focused {pane}."),
    ("rename", "renamed", "Renamed."),
    ("move_pane", "moved", "Moved {pane}."),
    ("swap_panes", "swapped", "Swapped the two panes."),
    ("resize_pane", "resized", "Resized {pane}."),
    ("scroll_pane", "scrolled", "Scrolled {pane}."),
    ("stack_panes", "stacked", "Stacked the panes."),
    ("rerun_pane", "rerun", "Re-ran {pane}."),
    ("send_input", "input sent", "Sent the input to {pane}."),
    (
        "agent_send",
        "input sent",
        "Sent the message to the agent in {pane}.",
    ),
    (
        "toggle_pane",
        "toggled fullscreen",
        "Toggled fullscreen on {pane}.",
    ),
    (
        "toggle_pane",
        "toggled floating",
        "Toggled floating on {pane}.",
    ),
    ("set_pane_pinned", "pinned", "Pinned {pane}."),
    ("set_pane_pinned", "unpinned", "Unpinned {pane}."),
    (
        "set_floating_pane_geometry",
        "geometry set",
        "Moved and resized {pane}.",
    ),
];

/// The sentence for a verb result, or `None` if this is not one — which covers
/// every error these tools return, and every tool not in the table.
fn ack(name: &str, result: &str, args: &Value) -> Option<String> {
    let sentence = ACKS
        .iter()
        .find(|(tool, success, _)| *tool == name && *success == result)
        .map(|(_, _, sentence)| *sentence)?;
    Some(sentence.replace("{pane}", &pane_phrase(args)))
}

/// How to name the pane a call acted on, given the arguments it was called with.
fn pane_phrase(args: &Value) -> String {
    match args.get("pane_id").and_then(Value::as_u64) {
        Some(id) => format!("pane {id}"),
        None => "the pane".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Single objects
// ---------------------------------------------------------------------------

fn opened_pane(value: &Value) -> Option<String> {
    // The id is in a code span because it is the thing to type back into the
    // next command, not a number to read past.
    flag(value, "opened").then(|| format!("Opened pane `{}`.\n", number(value, "pane_id")))
}

fn opened_tab(value: &Value) -> Option<String> {
    flag(value, "opened").then(|| format!("Opened tab `{}`.\n", number(value, "tab_id")))
}

/// The pane a spawned agent landed in, and the kind it was registered as if
/// `present` split one off the end of the result.
fn spawned(value: &Value, registered: Option<&str>) -> Option<String> {
    if !flag(value, "opened") {
        return None;
    }
    let mut out = format!("Spawned an agent in pane {}.\n", number(value, "pane_id"));
    if let Some(kind) = registered {
        out.push_str(&format!("Registered as {}.\n", inline(kind)));
    }
    Some(out)
}

fn todo_written(name: &str, value: &Value) -> Option<String> {
    let item = format!("`{}` {}", number(value, "id"), inline(&text(value, "text")));
    let project = text(value, "project");
    let mut out = match name {
        "todo_add" => format!("Added {item}"),
        "todo_edit" => format!("Edited {item}"),
        _ if flag(value, "done") => format!("Crossed off {item}"),
        _ => format!("Reopened {item}"),
    };
    if !project.is_empty() {
        out.push_str(&format!(" in {}", inline(&project)));
    }
    out.push_str(".\n");
    // Both are counts of items the write moved as a consequence, and a change
    // the user did not ask for by name is exactly what wants saying.
    for (key, label) in [
        ("cascaded", "also crossed off"),
        ("reopened", "also reopened"),
    ] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            if !items.is_empty() {
                let ids: Vec<String> = items.iter().map(|item| item.to_string()).collect();
                out.push_str(&format!("{label}: {}\n", ids.join(", ")));
            }
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Slack and the file tools
// ---------------------------------------------------------------------------

/// The `exit code: N\n\nstdout:\n…` envelope `on_command_result` wraps a shelled
/// command in, unwrapped — but only on success. A non-zero exit is a diagnostic
/// and is worth showing exactly as it came.
fn command_stdout(result: &str) -> Option<&str> {
    let rest = result.strip_prefix("exit code: 0")?;
    match rest.split_once("stdout:\n") {
        Some((_, stdout)) => Some(stdout.trim_end()),
        // Exit 0 with nothing on stdout: the command worked and said nothing.
        None => Some(""),
    }
}

fn slack(name: &str, result: &str) -> Option<String> {
    let stdout = command_stdout(result)?;
    if stdout.is_empty() {
        return Some(format!("{} finished.\n", name.replace('_', " ")));
    }
    // dobbs' own shape is its business and changes independently of us, so this
    // renders whatever JSON it produced rather than naming fields.
    match serde_json::from_str::<Value>(stdout) {
        Ok(value) if value.is_object() || value.is_array() => {
            Some(heading(name) + &generic(&value)?)
        },
        // Not JSON: dobbs printed prose, which is already what we want.
        _ => Some(heading(name) + &fence(stdout)),
    }
}

/// `rg` and `sed` output, which is already lines. Fenced rather than bulleted:
/// a match is a line of source, and its indentation is part of reading it.
fn file_tool(name: &str, args: &Value, result: &str) -> Option<String> {
    // `events::file_tool_result` turns these into their own sentences, and they
    // are better than anything wrapped around them.
    if matches!(result.trim(), "no matches" | "no lines") || result.starts_with("exit code:") {
        return None;
    }
    // The footer is ours, not the command's, and reads as prose rather than as
    // one more line of output.
    let (body, footer) = match result.split_once("\n\n… ") {
        Some((body, footer)) => (body, Some(footer)),
        None => (result, None),
    };

    let path = text(args, "path");
    let mut out = if path.is_empty() {
        heading(name)
    } else {
        heading(&format!("{name} — {path}"))
    };
    out.push_str(&fence(body));
    if let Some(footer) = footer {
        out.push_str(&format!("\n_{}_\n", inline(footer.trim())));
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// The generic renderer
// ---------------------------------------------------------------------------

/// Any JSON as nested markdown lists — the floor every tool gets for free.
///
/// Deliberately shape-driven rather than tool-driven, so a tool added to
/// `tools::specs` without an arm here still reads as a list of labelled values
/// instead of a line of braces.
fn generic(value: &Value) -> Option<String> {
    let body = render(value, 0);
    (!body.trim().is_empty()).then_some(body)
}

fn render(value: &Value, depth: usize) -> String {
    let pad = "  ".repeat(depth);
    match value {
        // Keys are not escaped: they are field names from our own tool JSON,
        // never user text, and `pane\_id` reads worse than it protects.
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| match leaf(value) {
                Some(text) => format!("{pad}- **{key}**: {text}\n"),
                None => format!("{pad}- **{key}**:\n{}", render(value, depth + 1)),
            })
            .collect(),
        Value::Array(items) if items.is_empty() => format!("{pad}- _none_\n"),
        Value::Array(items) => items
            .iter()
            .map(|item| match leaf(item) {
                Some(text) => format!("{pad}- {text}\n"),
                None => format!("{pad}-\n{}", render(item, depth + 1)),
            })
            .collect(),
        other => format!("{pad}- {}\n", leaf(other).unwrap_or_default()),
    }
}

/// A scalar as inline markdown, or `None` for a container that needs recursing.
fn leaf(value: &Value) -> Option<String> {
    match value {
        Value::Null => Some("_none_".to_owned()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) if text.is_empty() => Some("_empty_".to_owned()),
        Value::String(text) => Some(inline(text)),
        Value::Object(fields) if fields.is_empty() => Some("_none_".to_owned()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Escaping
// ---------------------------------------------------------------------------

/// Text safe to drop into a line of markdown.
///
/// Every string here came from somewhere we do not control — a pane title, a
/// branch, a todo, a Slack message. Backslash-escaping the characters the
/// renderer acts on is what stops one of them restyling the rest of the
/// transcript.
/// Not escaped, deliberately: `_`, `|` and `~`. An underscore inside a word is
/// literal in CommonMark, so escaping it would only turn every `pane_id` and
/// `slack_status` into `pane\_id`; `|` and `~` mean nothing with the table and
/// strikethrough extensions off, which they are. The worst any of them can do
/// is a stray italic, against a backslash on every identifier we print.
pub fn inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '`' | '*' | '[' | ']' | '<' | '>' | '#') {
            out.push('\\');
        }
        // A newline inside a value would end the list item it sits in.
        out.push(if ch == '\n' || ch == '\r' { ' ' } else { ch });
    }
    out
}

/// `body` in a fenced code block, with a fence long enough that `body` cannot
/// close it early.
///
/// The length matters: tool output routinely contains backticks — a shell
/// prompt, a Slack message, a line of source — and a three-backtick fence would
/// end there, spilling the rest into the transcript as markup.
pub fn fence(body: &str) -> String {
    let longest = body
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let ticks = "`".repeat(longest.max(2) + 1);
    format!("{ticks}\n{}\n{ticks}\n", body.trim_end_matches('\n'))
}

/// Rows as aligned columns inside a fenced block.
///
/// The substitute for a markdown table, which the renderer does not parse. The
/// fence also means nothing in a cell can restyle anything, so cells are *not*
/// passed through `inline` — they are shown literally, which is what a listing
/// of titles and commands wants.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(column, header)| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| display_width(cell))
                .chain(std::iter::once(header.chars().count()))
                .max()
                .unwrap_or_default()
        })
        .collect();

    let mut out = String::new();
    for row in std::iter::once(&headers.iter().map(|h| (*h).to_owned()).collect()).chain(rows) {
        let mut line = String::new();
        for (column, width) in widths.iter().enumerate() {
            let cell = row.get(column).map(String::as_str).unwrap_or_default();
            line.push_str(cell);
            // No padding after the last column: trailing spaces are invisible
            // and would only widen the block.
            if column + 1 < widths.len() {
                line.push_str(&" ".repeat(width.saturating_sub(display_width(cell)) + 2));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    fence(out.trim_end_matches('\n'))
}

/// Columns a cell occupies. Wide characters take two, which is what keeps a
/// CJK pane title from skewing every column to its right.
fn display_width(text: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    text.chars().map(|ch| ch.width().unwrap_or(0)).sum()
}

/// A markdown heading naming the command that produced the block below it.
///
/// This is what replaces the `fn:` prefix the plain path puts on a tool result:
/// it says which command answered, and the renderer draws it bold and coloured.
///
/// Escaped, because several callers interpolate something they were given — a
/// session name, a file path, a search query — and a newline in one would end
/// the heading and leave the rest as body text.
pub fn heading(name: &str) -> String {
    format!("## {}\n\n", inline(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: "manual-1".to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        }
    }

    // -- verbs --------------------------------------------------------------

    #[test]
    fn a_verb_becomes_a_sentence_naming_the_pane() {
        assert_eq!(
            present(&call("clear_pane", r#"{"pane_id":3}"#), "cleared"),
            Some("Cleared pane 3's scrollback.".to_owned())
        );
    }

    #[test]
    fn a_verb_without_a_pane_id_still_reads() {
        assert_eq!(
            present(&call("close_pane", "{}"), "closed"),
            Some("Closed the pane.".to_owned())
        );
    }

    /// The two `toggle_pane` results are different sentences, so the table has
    /// to be keyed on the result and not only on the tool.
    #[test]
    fn one_tool_with_two_results_says_two_different_things() {
        let call = call("toggle_pane", r#"{"pane_id":1}"#);
        assert_eq!(
            present(&call, "toggled floating"),
            Some("Toggled floating on pane 1.".to_owned())
        );
        assert_eq!(
            present(&call, "toggled fullscreen"),
            Some("Toggled fullscreen on pane 1.".to_owned())
        );
    }

    // -- falling back -------------------------------------------------------

    /// The whole safety property: anything we have nothing better to say about
    /// is shown exactly as it arrived.
    #[test]
    fn an_error_is_never_dressed_up_as_a_success() {
        for (name, result) in [
            ("clear_pane", "pane_id and is_plugin are both required"),
            ("close_pane", "no such tool: close_pane"),
            (
                "rerun_pane",
                "rerun_pane only supports terminal command panes",
            ),
            ("set_pane_pinned", "pinned is required"),
            ("agent_list", "unknown project x; call project_list"),
        ] {
            assert_eq!(present(&call(name, "{}"), result), None, "{name}: {result}");
        }
    }

    /// A verb tool whose result is *nearly* its success string is still a
    /// failure, and must not match.
    #[test]
    fn a_near_miss_of_a_success_string_does_not_match() {
        assert_eq!(present(&call("clear_pane", "{}"), "not cleared"), None);
        assert_eq!(present(&call("clear_pane", "{}"), "cleared."), None);
    }

    #[test]
    fn a_tool_that_already_speaks_prose_is_left_alone() {
        assert_eq!(
            present(
                &call("schedule", "{}"),
                "scheduled job 1 — runs every 60s until unscheduled"
            ),
            None
        );
    }

    #[test]
    fn malformed_json_falls_back_rather_than_erroring() {
        assert_eq!(present(&call("list_session", "{}"), "{not json"), None);
    }

    /// A bare scalar is valid JSON but not a structure worth rendering.
    #[test]
    fn a_scalar_result_is_not_treated_as_structure() {
        assert_eq!(present(&call("whatever", "{}"), "42"), None);
        assert_eq!(present(&call("whatever", "{}"), "\"hello\""), None);
    }

    // -- the generic renderer ----------------------------------------------

    #[test]
    fn an_unknown_tool_still_gets_a_readable_list() {
        let result = present(
            &call("some_future_tool", "{}"),
            r#"{"opened":true,"pane_id":7}"#,
        );
        assert_eq!(
            result,
            Some("- **opened**: true\n- **pane_id**: 7\n".to_owned())
        );
    }

    #[test]
    fn nested_objects_become_nested_lists() {
        let raw = r#"{"tabs":[{"name":"one","panes":[]}]}"#;
        assert_eq!(
            present(&call("some_future_tool", "{}"), raw),
            Some(
                "- **tabs**:\n  -\n    - **name**: one\n    - **panes**:\n      - _none_\n"
                    .to_owned()
            )
        );
    }

    // -- escaping -----------------------------------------------------------

    /// A pane title is not ours, and markdown in one must not reach the
    /// renderer as markup.
    #[test]
    fn markup_in_a_value_is_escaped() {
        let raw = r##"{"title":"# heading *bold*"}"##;
        let shown = present(&call("some_future_tool", "{}"), raw).expect("presented");
        assert!(shown.contains(r"\# heading \*bold\*"), "{shown}");
    }

    #[test]
    fn a_newline_in_a_value_cannot_break_the_list_item() {
        let raw = r#"{"title":"one\ntwo"}"#;
        let shown = present(&call("some_future_tool", "{}"), raw).expect("presented");
        assert_eq!(shown, "- **title**: one two\n");
    }

    /// The deliberate half of the escaping rule: an identifier must survive
    /// intact, or every listing is full of backslashes.
    #[test]
    fn an_underscore_is_left_alone() {
        assert_eq!(inline("pane_id"), "pane_id");
        assert_eq!(inline("a|b~c"), "a|b~c");
    }

    /// A backtick in a value must not be able to open a code span that swallows
    /// the rest of the line.
    #[test]
    fn a_backtick_in_a_value_is_escaped() {
        assert_eq!(inline("a `b` c"), r"a \`b\` c");
    }

    #[test]
    fn fence_survives_a_body_containing_backticks() {
        let body = "run ``` then ```` more";
        let fenced = fence(body);
        let ticks = fenced.lines().next().expect("a fence line");
        assert!(
            ticks.len() > 4,
            "fence {ticks:?} is not longer than the body's longest run"
        );
        assert!(fenced.contains(body));
    }

    #[test]
    fn fence_of_a_plain_body_is_the_usual_three() {
        assert_eq!(fence("hello"), "```\nhello\n```\n");
    }

    // -- table --------------------------------------------------------------

    #[test]
    fn table_aligns_its_columns() {
        let rows = vec![
            vec!["1".to_owned(), "short".to_owned()],
            vec!["22".to_owned(), "longer".to_owned()],
        ];
        assert_eq!(
            table(&["id", "title"], &rows),
            "```\nid  title\n1   short\n22  longer\n```\n"
        );
    }

    /// A CJK title is two columns per character; measuring it in `char`s would
    /// skew every column to its right.
    #[test]
    fn table_measures_wide_characters_by_display_width() {
        let rows = vec![
            vec!["日本".to_owned(), "a".to_owned()],
            vec!["ab".to_owned(), "b".to_owned()],
        ];
        let rendered = table(&["name", "x"], &rows);
        for line in rendered.lines().filter(|line| !line.starts_with("```")) {
            let column = line.find(['a', 'b', 'x']).map(|_| ());
            assert!(column.is_some(), "{line:?}");
        }
        assert!(rendered.contains("日本  a"), "{rendered}");
    }

    #[test]
    fn table_of_no_rows_is_nothing_at_all() {
        assert_eq!(table(&["id"], &[]), "");
    }

    // -- the command list ---------------------------------------------------

    /// Grouping is only useful if it is complete: every command has to appear,
    /// and none twice.
    #[test]
    fn the_command_list_holds_every_command_exactly_once() {
        let listed: Vec<String> = commands()
            .split_whitespace()
            .filter_map(|word| word.strip_prefix('/'))
            .map(str::to_owned)
            .collect();
        let mut expected: Vec<String> = crate::LOCAL_COMMANDS
            .iter()
            .map(|name| (*name).to_owned())
            .chain(tools::names())
            .collect();
        let mut sorted = listed.clone();
        sorted.sort();
        expected.sort();
        assert_eq!(sorted, expected);
        assert_eq!(
            listed.len(),
            expected.len(),
            "a command was listed under two families"
        );
    }

    #[test]
    fn the_command_list_is_grouped_and_plain() {
        let listed = commands();
        for family in ["hocus", "panes", "agents", "todos", "slack"] {
            assert!(listed.contains(family), "{family} missing from {listed}");
        }
        // A notice is not rendered as markdown, so any syntax would be shown
        // literally.
        assert!(!listed.contains('`') && !listed.contains('#'), "{listed}");
    }

    /// `agent_send` must land under agents, not be dragged into panes by a
    /// substring match on `send`.
    #[test]
    fn a_family_matches_whole_words_not_substrings() {
        assert!(in_family("agent_send", "agent"));
        assert!(!in_family("agent_send", "pane"));
        assert!(in_family("close_pane", "pane"));
        assert!(in_family("rename", "rename"));
    }

    // -- coverage -----------------------------------------------------------

    /// Every advertised tool reaches an arm that answers, one way or the other.
    ///
    /// A `match` on `&str` cannot be exhaustive, so this is what stops a tool
    /// added to `tools::specs` from quietly going back to showing raw JSON. It
    /// runs each tool's plausible answers through and asserts none of them
    /// panics and that a structured one is never left unpresented.
    #[test]
    fn every_tool_is_covered() {
        for name in tools::names() {
            let call = call(&name, r#"{"pane_id":1,"path":"x"}"#);
            // Whatever it answers, an error is passed through. Not asserted for
            // the file tools: their output is arbitrary text from `rg`/`sed`,
            // so no string is distinguishable from a legitimate answer. Their
            // real failure shapes are the ones `file_tool` checks for, and
            // `an_empty_search_keeps_its_own_wording` covers those.
            if !tools::FILE_TOOLS.contains(&name.as_str()) {
                assert_eq!(
                    present(&call, "something went wrong"),
                    None,
                    "{name} dressed up an error"
                );
            }
            // And anything structured is always turned into something, whether
            // by its own arm or by the generic renderer.
            // A shape no arm recognises still reaches the generic renderer,
            // except for the tools that answer in prose on purpose.
            let verbatim = PROSE.contains(&name.as_str())
                || tools::SLACK_TOOLS.contains(&name.as_str())
                || tools::FILE_TOOLS.contains(&name.as_str());
            assert!(
                present(&call, r#"{"a":1}"#).is_some() || verbatim,
                "{name} left structured output unpresented"
            );
        }
    }

    // -- listings -----------------------------------------------------------

    /// The shape `tools::list_session` builds.
    #[test]
    fn a_session_listing_becomes_tabs_of_panes() {
        let raw = r#"{"session":"work","tabs":[{"tab_number":1,"tab_id":1,"name":"edit",
            "active":true,"panes":[
              {"pane_id":3,"is_plugin":false,"title":"vim","focused":true,"floating":false,
               "command":"vim","exit_status":null},
              {"pane_id":4,"is_plugin":false,"title":"build","focused":false,"floating":false,
               "command":"cargo","exit_status":1}]}]}"#;
        let shown = present(&call("list_session", "{}"), raw).expect("presented");
        assert!(shown.starts_with("## session work\n\n"), "{shown}");
        // The heading carries the number the tab bar shows, not the position.
        assert!(shown.contains("**1: edit** — active"), "{shown}");
        // The ids lead, because they are what the next command needs, and the
        // columns line up under them.
        assert!(shown.contains("3   *  vim    running"), "{shown}");
        assert!(shown.contains("4      build  exited 1"), "{shown}");
        // The marks column is only readable with a key, and only one pane here
        // is marked at all.
        assert!(shown.contains("_* focused_"), "{shown}");
        assert!(!shown.contains("floating"), "{shown}");
    }

    #[test]
    fn a_session_with_no_tabs_says_so_rather_than_showing_braces() {
        let shown = present(&call("list_session", "{}"), r#"{"session":"w","tabs":[]}"#);
        assert_eq!(shown, Some("## session w\n\n_no tabs_\n".to_owned()));
    }

    /// `agents::list_json` answers with a bare array, and in pane order — so the
    /// grouping has to be built here, not assumed.
    #[test]
    fn agents_group_under_their_project_and_branch() {
        let raw = r#"[
            {"pane_id":1,"status":"running","title":"one","project":"a","branch":"main",
             "idle_seconds":0},
            {"pane_id":2,"status":"running","title":"two","project":"b","branch":"topic",
             "idle_seconds":180},
            {"pane_id":3,"status":"running","title":"three","project":"a","branch":"main"}]"#;
        let shown = present(&call("agent_list", "{}"), raw).expect("presented");
        assert!(shown.contains("**a** · main"), "{shown}");
        assert!(shown.contains("**b** · topic"), "{shown}");
        // Interleaved input, one header each.
        assert_eq!(shown.matches("**a** · main").count(), 1, "{shown}");
        assert!(shown.contains("`1` busy — one"), "{shown}");
        assert!(shown.contains("`2` idle 3m — two"), "{shown}");
        // No `idle_seconds` yet means the poll has not seen it.
        assert!(shown.contains("`3` running — three"), "{shown}");
    }

    #[test]
    fn an_agent_without_a_project_still_gets_a_header() {
        let raw = r#"[{"pane_id":1,"status":"exited","title":"x"}]"#;
        let shown = present(&call("agent_list", "{}"), raw).expect("presented");
        assert!(shown.contains("**no project** · no branch"), "{shown}");
        assert!(shown.contains("`1` exited — x"), "{shown}");
    }

    #[test]
    fn no_agents_says_none() {
        let shown = present(&call("agent_list", "{}"), "[]");
        assert_eq!(shown, Some("## agents\n\n_none_\n".to_owned()));
    }

    /// The shape `todos::list_json` builds, including the child nesting.
    #[test]
    fn todos_become_a_nested_checklist_keeping_their_ids() {
        let raw = r#"{"project":"hocus","root":"/w/h","open":2,"done":1,
            "items":[{"id":1,"text":"parent","children":[{"id":2,"text":"child"}]}]}"#;
        let shown = present(&call("todo_list", "{}"), raw).expect("presented");
        assert!(shown.starts_with("## todos — hocus\n\n"), "{shown}");
        assert!(shown.contains("2 open, 1 done"), "{shown}");
        assert!(shown.contains("- [ ] `1` parent\n"), "{shown}");
        assert!(shown.contains("  - [ ] `2` child\n"), "{shown}");
    }

    #[test]
    fn a_done_todo_is_ticked() {
        let raw = r#"{"project":"h","open":0,"done":1,
            "items":[{"id":1,"text":"t","done":true}]}"#;
        let shown = present(&call("todo_list", "{}"), raw).expect("presented");
        assert!(shown.contains("- [x] `1` t"), "{shown}");
    }

    #[test]
    fn a_project_listing_marks_the_active_one() {
        let raw = r#"{"active":"hocus","projects":[
            {"name":"hocus","root":"/w/h","tab_ids":[1,2]},
            {"name":"other","root":"/w/o","tab_ids":[]}]}"#;
        let shown = present(&call("project_list", "{}"), raw).expect("presented");
        assert!(shown.contains("- **hocus** — active"), "{shown}");
        assert!(shown.contains("- **other**\n"), "{shown}");
        assert!(shown.contains("  - tabs 1, 2"), "{shown}");
    }

    #[test]
    fn orphans_report_an_age_a_person_reads() {
        let raw = r#"{"orphans":[{"host_id":"abc","command":"top","session":"old",
            "exited":false,"age_seconds":7200}]}"#;
        let shown = present(&call("list_orphaned_panes", "{}"), raw).expect("presented");
        assert!(shown.contains("abc"), "{shown}");
        assert!(shown.contains("2h"), "{shown}");
    }

    // -- single objects -----------------------------------------------------

    #[test]
    fn opening_a_pane_reports_the_id_the_next_command_needs() {
        let raw = r#"{"opened":true,"pane_id":7,"is_plugin":false}"#;
        let shown = present(&call("new_pane", "{}"), raw).expect("presented");
        assert!(shown.contains('7'), "{shown}");
    }

    /// `register_spawned` glues its note onto `new_pane`'s JSON before the
    /// result ever reaches us, so the suffix has to be split back off.
    #[test]
    fn a_spawned_agent_reports_the_kind_it_was_registered_as() {
        let raw = r#"{"opened":true,"pane_id":9} (registered as claude)"#;
        assert_eq!(
            present(&call("agent_spawn", "{}"), raw),
            Some("Spawned an agent in pane 9.\nRegistered as claude.\n".to_owned())
        );
    }

    #[test]
    fn a_spawn_without_a_registration_still_reads() {
        let raw = r#"{"opened":true,"pane_id":9}"#;
        assert_eq!(
            present(&call("agent_spawn", "{}"), raw),
            Some("Spawned an agent in pane 9.\n".to_owned())
        );
    }

    #[test]
    fn crossing_off_a_todo_names_what_else_it_moved() {
        let raw = r#"{"project":"h","id":2,"text":"child","done":true,
            "cascaded":[3,4],"reopened":[]}"#;
        let shown = present(&call("todo_done", "{}"), raw).expect("presented");
        assert!(
            shown.starts_with("Crossed off `2` child in h.\n"),
            "{shown}"
        );
        assert!(shown.contains("also crossed off: 3, 4"), "{shown}");
        assert!(!shown.contains("also reopened"), "{shown}");
    }

    #[test]
    fn undoing_a_todo_says_reopened_rather_than_crossed_off() {
        let raw = r#"{"project":"h","id":2,"text":"t","done":false}"#;
        let shown = present(&call("todo_done", "{}"), raw).expect("presented");
        assert!(shown.starts_with("Reopened `2` t in h."), "{shown}");
    }

    // -- slack and files ----------------------------------------------------

    /// A non-zero exit is a diagnostic; wrapping it would only bury it.
    #[test]
    fn a_failed_command_is_shown_exactly_as_it_arrived() {
        let raw = "exit code: 2\n\nstderr:\ndobbs: no such channel";
        assert_eq!(present(&call("slack_read", "{}"), raw), None);
    }

    #[test]
    fn a_slack_payload_is_unwrapped_from_its_exit_envelope() {
        let raw = "exit code: 0\n\nstdout:\n{\"ok\":true}";
        let shown = present(&call("slack_status", "{}"), raw).expect("presented");
        assert!(shown.starts_with("## slack_status\n\n"), "{shown}");
        assert!(shown.contains("- **ok**: true"), "{shown}");
        assert!(!shown.contains("exit code"), "{shown}");
    }

    #[test]
    fn a_silent_success_says_it_finished() {
        assert_eq!(
            present(&call("slack_post", "{}"), "exit code: 0"),
            Some("slack post finished.\n".to_owned())
        );
    }

    #[test]
    fn file_output_is_fenced_so_its_indentation_survives() {
        let raw = "src/main.rs:1:    fn main() {";
        let shown =
            present(&call("read_file", r#"{"path":"src/main.rs"}"#), raw).expect("presented");
        assert!(
            shown.starts_with("## read_file — src/main.rs\n\n"),
            "{shown}"
        );
        assert!(shown.contains("    fn main() {"), "{shown}");
    }

    /// `events::file_tool_result` already words these, and better.
    #[test]
    fn an_empty_search_keeps_its_own_wording() {
        assert_eq!(present(&call("find_files", "{}"), "no matches"), None);
        assert_eq!(present(&call("read_file", "{}"), "no lines"), None);
    }

    #[test]
    fn the_truncation_footer_becomes_prose_rather_than_another_line() {
        let raw = "a.rs\nb.rs\n\n… 2 of 90 lines shown; narrow the search.";
        let shown = present(&call("find_files", "{}"), raw).expect("presented");
        assert!(
            shown.contains("_2 of 90 lines shown; narrow the search._"),
            "{shown}"
        );
        // The footer is out of the fenced block, not inside it.
        assert!(!shown.contains("```\n\n… "), "{shown}");
    }

    /// A listing is the likeliest place for a hostile pane title to land, and
    /// the table is fenced precisely so it cannot matter.
    #[test]
    fn a_pane_title_full_of_markup_cannot_restyle_the_transcript() {
        let raw = r##"{"session":"s","tabs":[{"tab_number":1,"tab_id":1,"name":"t","active":false,
            "panes":[{"pane_id":1,"title":"# **x** ```","focused":false}]}]}"##;
        let shown = present(&call("list_session", "{}"), raw).expect("presented");
        let fence_line = shown
            .lines()
            .find(|line| line.starts_with("```"))
            .expect("a fence");
        assert!(
            fence_line.len() > 3,
            "the title's own backticks must not close the fence: {fence_line:?}"
        );
    }
}

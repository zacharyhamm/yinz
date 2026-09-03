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
        "note_list" => note_list(&json),
        "note_get" => note_get(&json),
        "note_search" => note_search(&json),
        "note_add" | "note_edit" | "note_archive" | "note_delete" | "note_link" => {
            note_written(name, &json)
        },
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
    "linear_search",
    "linear_issues",
    "linear_issue",
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
/// `warp_tab_here` under tabs rather than letting the pane family's `warp` take
/// it, so the narrow families come before the broad ones.
const FAMILIES: &[(&str, &[&str])] = &[
    ("session", &["list_session", "list_other_sessions"]),
    ("tabs", &["new_tab", "close_tab", "warp_tab_here"]),
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
    ("notes", &["note"]),
    ("files", &["find_files", "search_files", "read_file"]),
    ("git", &["git"]),
    ("github", &["github"]),
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
#[cfg(test)]
pub fn commands() -> String {
    commands_with(Vec::new(), Vec::new())
}

/// `extra` are the extension tools, grouped with the rest by family. `skills`
/// get a group of their own, never bucketed: a skill named `pane` is not a
/// pane tool, and the list would say it was.
pub fn commands_with(extra: Vec<String>, skills: Vec<String>) -> String {
    let mut groups: Vec<(String, Vec<String>)> = vec![(
        "hocus".to_owned(),
        crate::LOCAL_COMMANDS
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
    )];
    if !skills.is_empty() {
        groups.push(("skills".to_owned(), skills));
    }

    let mut all = tools::names();
    all.extend(extra);
    all.sort();
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

/// `/skills`: every loaded skill with its description and tier, then what was
/// found but refused, then what went wrong reading a folder. Plain text, for
/// the reason `commands_with` is — and a description is the author's text,
/// which a `#` or `*` in must not restyle.
pub fn skills_listing(registry: &crate::skills::Registry, user_dir: &str) -> String {
    let mut out = String::new();
    let loaded = registry.all();
    if loaded.is_empty() {
        let user_dir = if user_dir.is_empty() {
            "<user plugin dir>/hocus/skills".to_owned()
        } else {
            user_dir.to_owned()
        };
        out.push_str(&format!(
            "no skills loaded. A skill is <name>/SKILL.md under {user_dir} or under \
             <project root>/.hocus/skills; /skills reload reads the folders again."
        ));
    } else {
        let width = loaded
            .iter()
            .map(|skill| skill.name.chars().count() + 1)
            .max()
            .unwrap_or_default();
        for skill in &loaded {
            let command = format!("/{}", skill.name);
            let state = if !skill.enabled {
                ", disabled"
            } else if registry.is_shadowed(skill) {
                ", shadowed"
            } else {
                ""
            };
            out.push_str(&format!(
                "{command:<width$}  {}  ({}{state})\n",
                skill.description,
                skill.source.label()
            ));
            for warning in &skill.warnings {
                out.push_str(&format!("{:width$}  ! {warning}\n", ""));
            }
        }
    }
    let skipped = registry.skipped();
    if !skipped.is_empty() {
        out.push_str("\nnot loaded:\n");
        for (source, name, why) in skipped {
            out.push_str(&format!("  {name} ({}): {why}\n", source.label()));
        }
    }
    let errors = registry.errors();
    if !errors.is_empty() {
        out.push_str("\nerrors:\n");
        for (source, error) in errors {
            out.push_str(&format!("  {}: {error}\n", source.label()));
        }
    }
    out.trim_end().to_owned()
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
        ("hidden", "h", "hidden"),
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
    // An agent that reports for itself has already said which word this is - see
    // `agents::status_cell`. Deriving one from `idle_seconds` here as well would be a
    // third place deciding busy from idle, and the one most likely to disagree.
    let state = text(agent, "state");
    if !state.is_empty() {
        return state;
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
        let indent = "  ".repeat(depth);
        out.push_str(&format!("{indent}- {box_} `{}`", number(item, "id")));
        // The notes about an item, by id, so the reader can `/note_get` one.
        let notes = array(item, "notes");
        if !notes.is_empty() {
            out.push_str(&format!(" · notes {}", ids(notes)));
        }
        out.push('\n');
        markdown_body(&text(item, "text"), &format!("{indent}  "), out);
        todo_items(array(item, "children"), depth + 1, out);
    }
}

fn markdown_body(text: &str, indent: &str, out: &mut String) {
    out.push('\n');
    for line in text.split('\n') {
        out.push_str(indent);
        out.push_str(line);
        out.push('\n');
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
            "- {box_} `{}` — _{}_\n",
            number(found, "id"),
            inline(&project)
        ));
        markdown_body(&text(found, "text"), "  ", &mut out);
    }
    if flag(value, "truncated") {
        out.push_str("\n_more matches were cut; narrow the query_\n");
    }
    Some(out)
}

/// Numbers joined for prose: `3, 7`.
fn ids(values: &[Value]) -> String {
    values
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The line a note gets in a listing: its id, title, age, and whatever else it
/// carries. The body is deliberately not here; `note_get` is how one is read.
fn note_line(note: &Value) -> String {
    let mut line = format!(
        "- `{}` **{}**",
        number(note, "id"),
        inline(&text(note, "title"))
    );
    if flag(note, "archived") {
        line.push_str(" _(archived)_");
    }
    let mut extras: Vec<String> = Vec::new();
    let age = age(note.get("age_seconds").and_then(Value::as_u64));
    if !age.is_empty() {
        extras.push(age);
    }
    let tags = array(note, "tags");
    if !tags.is_empty() {
        extras.push(
            tags.iter()
                .map(|tag| format!("#{}", inline(tag.as_str().unwrap_or_default())))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    let todos = array(note, "todo_ids");
    if !todos.is_empty() {
        extras.push(format!("todos {}", ids(todos)));
    }
    let author = text(note, "author");
    if !author.is_empty() && author != "user" {
        extras.push(format!("by {}", inline(&author)));
    }
    if !extras.is_empty() {
        line.push_str(" — ");
        line.push_str(&extras.join(" · "));
    }
    line.push('\n');
    line
}

fn note_list(value: &Value) -> Option<String> {
    let mut out = heading(&format!("notes — {}", text(value, "scope")));
    let notes = array(value, "notes");
    if notes.is_empty() {
        return Some(out + "_none_\n");
    }
    out.push_str(&format!("{} notes\n\n", notes.len()));
    for note in notes {
        out.push_str(&note_line(note));
    }
    Some(out)
}

fn note_get(value: &Value) -> Option<String> {
    let mut out = heading(&format!(
        "note {} — {}",
        number(value, "id"),
        text(value, "title")
    ));
    let mut meta: Vec<String> = Vec::new();
    match text(value, "project") {
        project if project.is_empty() && flag(value, "global") => meta.push("global".to_owned()),
        project if project.is_empty() => meta.push(inline(&text(value, "root"))),
        project => meta.push(inline(&project)),
    }
    let author = text(value, "author");
    if !author.is_empty() {
        meta.push(format!("by {}", inline(&author)));
    }
    let age = age(value.get("age_seconds").and_then(Value::as_u64));
    if !age.is_empty() {
        meta.push(age);
    }
    let tags = array(value, "tags");
    if !tags.is_empty() {
        meta.push(
            tags.iter()
                .map(|tag| format!("#{}", inline(tag.as_str().unwrap_or_default())))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    let todos = array(value, "todo_ids");
    if !todos.is_empty() {
        meta.push(format!("todos {}", ids(todos)));
    }
    if flag(value, "archived") {
        meta.push("archived".to_owned());
    }
    out.push_str(&format!("_{}_\n", meta.join(" · ")));
    markdown_body(&text(value, "body"), "", &mut out);
    Some(out)
}

fn note_search(value: &Value) -> Option<String> {
    let matches = array(value, "matches");
    let mut out = heading(&format!("note search — {}", text(value, "query")));
    if matches.is_empty() {
        return Some(out + "_no matches_\n");
    }
    for found in matches {
        out.push_str(&note_line(found));
        let snippet = text(found, "snippet");
        if !snippet.is_empty() {
            out.push_str(&format!("  {}\n", inline(&snippet)));
        }
    }
    if flag(value, "truncated") {
        out.push_str("\n_more matches were cut; narrow the query_\n");
    }
    Some(out)
}

fn note_written(name: &str, value: &Value) -> Option<String> {
    if name == "note_link" {
        let verb = if flag(value, "linked") {
            "Linked"
        } else {
            "Unlinked"
        };
        return Some(format!(
            "{verb} note `{}` and todo `{}`.\n",
            number(value, "note_id"),
            number(value, "todo_id")
        ));
    }
    let note = format!("note `{}`", number(value, "id"));
    let mut out = match name {
        "note_add" => format!("Added {note}"),
        "note_edit" => format!("Edited {note}"),
        "note_delete" => format!("Deleted {note}"),
        _ if flag(value, "archived") => format!("Archived {note}"),
        _ => format!("Unarchived {note}"),
    };
    let project = text(value, "project");
    if !project.is_empty() {
        out.push_str(&format!(" in {}", inline(&project)));
    } else if flag(value, "global") {
        out.push_str(" (global)");
    }
    out.push_str(".\n");
    let title = text(value, "title");
    if !title.is_empty() {
        out.push_str(&format!("\n**{}**\n", inline(&title)));
    }
    let tags = array(value, "tags");
    if !tags.is_empty() {
        out.push_str(&format!(
            "tags: {}\n",
            tags.iter()
                .map(|tag| format!("#{}", inline(tag.as_str().unwrap_or_default())))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    let todos = array(value, "todo_ids");
    if !todos.is_empty() {
        out.push_str(&format!("todos: {}\n", ids(todos)));
    }
    let warning = text(value, "warning");
    if !warning.is_empty() {
        out.push_str(&format!("\n_{}_\n", inline(&warning)));
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
    // Only the git path sets these, and it is the path that wrote to the user's
    // repository — so where it put the agent is the part worth reading back.
    let branch = text(value, "branch");
    if !branch.is_empty() {
        let cwd = text(value, "cwd");
        if cwd.is_empty() {
            out.push_str(&format!("On a new branch {}.\n", inline(&branch)));
        } else {
            out.push_str(&format!(
                "On a new branch {} in {}.\n",
                inline(&branch),
                inline(&cwd)
            ));
        }
    }
    if let Some(kind) = registered {
        out.push_str(&format!("Registered as {}.\n", inline(kind)));
    }
    Some(out)
}

fn todo_written(name: &str, value: &Value) -> Option<String> {
    let item = format!("`{}`", number(value, "id"));
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
    out.push('\n');
    out.push_str(&text(value, "text"));
    out.push('\n');
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

#[path = "./unit/present_tests/mod.rs"]
#[cfg(test)]
mod present_tests;

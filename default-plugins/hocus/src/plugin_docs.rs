//! The plugin reference, served to the model as a tool.
//!
//! SDK declarations and worked examples are compiled into this tool.
//!
//! It is sectioned for cost rather than for size. Every section fits inside `MAX_TOOL_OUTPUT`
//! several times over, but a tool result stays in the transcript and is re-sent on every later
//! turn of the conversation — so handing the reference over whole would tax every message that
//! followed it. Asking for a section is how the model pays only for what it needs.
//!
use crate::plugin_source::{self, Search};
use crate::tools;

/// How much of a `plugin_docs` answer reaches the model.
///
/// Well above every section as they stand - the largest is an SDK part at ~13 KB - and that is
/// the point: this is not the mechanism, `SECTIONS` is. It is the backstop that keeps a reference
/// section which grows past what a tool call should cost from silently landing whole in a
/// transcript that then re-sends it on every later turn.
pub const MAX_DOCS_OUTPUT: usize = 16_000;

const EXAMPLE_CLOCK: &str = include_str!("../../../example-plugins/clock.ts");
const EXAMPLE_PLAYGROUND: &str = include_str!("../../../example-plugins/playground.ts");
const EXAMPLE_AGENT_COMMIT: &str = include_str!("../../../example-plugins/agent-commit.ts");
const EXAMPLE_DASHBOARD: &str = include_str!("../../../example-plugins/dashboard.ts");
const EXAMPLE_WORKFLOW: &str = include_str!("../../../example-plugins/workflow.ts");
const EXAMPLE_TODO_EXTENSION: &str = include_str!("../../../example-plugins/todo-extension.ts");
const EXAMPLE_HOCUS_COMPLETE: &str = include_str!("../../../example-plugins/hocus-complete.ts");
const EXAMPLE_TODO_PANE: &str = include_str!("../../../example-plugins/todo-pane.ts");
const EXAMPLE_HOCUS_TODOS: &str = include_str!("../../../example-plugins/hocus-todos.ts");
const EXAMPLE_HOCUS_NOTES: &str = include_str!("../../../example-plugins/hocus-notes.ts");
const EXAMPLE_CAPABILITIES: &str = include_str!("../../../example-plugins/capabilities.ts");
const EXAMPLE_AGENT_HARNESS: &str = include_str!("../../../example-plugins/agent-harness.ts");
const EXAMPLE_FORMS: &str = include_str!("../../../example-plugins/forms.ts");
const EXAMPLE_KEYBINDINGS: &str = include_str!("../../../example-plugins/keybindings.ts");
const EXAMPLE_WEATHER_EXTENSION: &str =
    include_str!("../../../example-plugins/weather-extension.ts");

/// The TypeScript API itself, verbatim: every command, reply, event and helper with its type,
/// so a model that has read the part it needs does not have to guess how a call is spelled.
/// Served in parts, cut at its own `// ---- section:` markers, so that none is more than a
/// tool call should cost.
const SDK: &str = include_str!("../../../zellij-plugin-host/src/deno/sdk.d.ts");
const SDK_SECTIONS: [&str; 7] = [
    "sdk_core",
    "sdk_commands",
    "sdk_replies",
    "sdk_types",
    "sdk_types_data",
    "sdk_actions",
    "sdk_helpers",
];

fn sdk_marker(name: &str) -> String {
    format!("// ---- section: {name}\n")
}

/// The part of the declarations between `name`'s marker and the next one. A missing marker is
/// `None`, which the tests turn into a failure rather than serving the wrong text.
fn sdk_part(name: &str) -> Option<&'static str> {
    let index = SDK_SECTIONS.iter().position(|part| *part == name)?;
    let start = SDK.find(&sdk_marker(name))?;
    let end = SDK_SECTIONS
        .get(index + 1)
        .and_then(|next| SDK.find(&sdk_marker(next)))
        .unwrap_or(SDK.len());
    Some(&SDK[start..end])
}

/// The section names the schema advertises, and the only ones `section` accepts.
pub const SECTIONS: [&str; 25] = [
    "index",
    "quickstart",
    "sdk_testing",
    "sdk_core",
    "sdk_commands",
    "sdk_replies",
    "sdk_types",
    "sdk_types_data",
    "sdk_actions",
    "sdk_helpers",
    "example_clock",
    "example_capabilities",
    "example_forms",
    "example_keybindings",
    "example_weather_extension",
    "example_agent_harness",
    "example_playground",
    "example_agent_commit",
    "example_dashboard",
    "example_workflow",
    "example_todo_extension",
    "example_hocus_complete",
    "example_todo_pane",
    "example_hocus_todos",
    "example_hocus_notes",
];

/// The available declarations and examples, with their sizes.
fn index() -> String {
    let mut out = String::from(
        "Zellij TypeScript plugin reference. Never ask another agent to write a plugin. \
         `plugin_write` formats and checks strict types and lint, asks approval, keeps a backup, writes the prepared source, then grants its literal permissions. \
         `plugin_check` checks saved files or drafts without writing. `plugin_format` formats drafts or confirms a saved-file change. Fix diagnostics and create dependencies first. To edit, use `plugin_outline` or \
         `plugin_search`, then `plugin_read` and `plugin_edit`. `plugin_reload` reports the first render \
         or load error; `plugin_log` reads logs. `plugin_docs query=<regex>` searches sections \
         (section:LINE:text); `section=` reads one, narrowed by `start_line`/`line_count`:\n\n",
    );
    for section in SECTIONS {
        let description = match section {
            "index" => continue,
            "quickstart" => "a complete minimal TypeScript plugin",
            "sdk_testing" => "declarations: assertions, harness, disposable sessions",
            "sdk_core" => "declarations: events, Plugin, the Zellij interface",
            "sdk_commands" => "declarations: every command's arguments",
            "sdk_replies" => "declarations: replies, zellij.host, keybindings",
            "sdk_types" => "declarations: input, layout, pane/tab/mode/session",
            "sdk_types_data" => "declarations: style, menus, agents, hooks, queues, data",
            "sdk_actions" => "declarations: Action",
            "sdk_helpers" => "declarations: fs, data, rpc, hocus, ui, agents, sequences",
            "example_clock" => "example: the small one",
            "example_capabilities" => "example: most of what a plugin can do, in one pane",
            "example_forms" => "example: shared forms, confirmation and scrollable Markdown",
            "example_keybindings" => "example: default keybindings",
            "example_weather_extension" => "example: a Hocus extension",
            "example_agent_harness" => "example: agent harness and verification",
            "example_playground" => "example: every shape of command in use",
            "example_agent_commit" => "example: an agent_exit hook",
            "example_dashboard" => "example: dashboard, timers, commands and input",
            "example_workflow" => "example: a multi-step command sequence in one pane",
            "example_todo_extension" => "example: Hocus tools with shared data",
            "example_hocus_complete" => "example: model completion with tools",
            "example_todo_pane" => "example: shared data watch and input",
            "example_hocus_todos" => {
                "example: reading hocus's own todo rows out of the shared store"
            },
            "example_hocus_notes" => "example: hocus's notes, tags and todo links",
            _ => continue,
        };
        // The size, so the choice can be made on what it costs. Every one of these lands in the
        // transcript and is re-sent on every later turn, and the range is 700 bytes to 13 KB -
        // wide enough that "read the relevant sections" is not advice a model can act on
        // without knowing which ones are cheap.
        let size = self::section(section).map_or(0, |text| text.len());
        out.push_str(&format!("- {section} (~{}k): {description}\n", size / 1000));
    }
    // The quickstart used to be appended here in full - 3.8 KB nobody asked for, on the first
    // call of every plugin conversation and on every round after it. It is one line above.
    out.push_str("\nStart with `quickstart` for TypeScript. Use explicit `.ts` filenames. Extensionless names select TypeScript.\n");
    out
}

/// The text for a section, or `None` if nothing goes by that name.
pub fn section(name: &str) -> Option<String> {
    match name {
        "index" => Some(index()),
        "sdk_testing" => Some(fenced(include_str!("../../../zellij-plugin-host/src/deno/testing/testing.d.ts"))),
        "quickstart" => Some(format!("Write `clock.ts` with `plugin_write`, then call `plugin_reload name=clock.ts`. Use explicit `.ts` filenames. Extensionless names select TypeScript.\n\n```ts\n{EXAMPLE_CLOCK}\n```\n\nRead `sdk_core` and `sdk_helpers` for the API declarations.")),
        "example_clock" => Some(fenced(EXAMPLE_CLOCK)),
        "example_capabilities" => Some(fenced(EXAMPLE_CAPABILITIES)),
        "example_forms" => Some(fenced(EXAMPLE_FORMS)),
        "example_keybindings" => Some(fenced(EXAMPLE_KEYBINDINGS)),
        "example_weather_extension" => Some(fenced(EXAMPLE_WEATHER_EXTENSION)),
        "example_agent_harness" => Some(fenced(EXAMPLE_AGENT_HARNESS)),
        "example_playground" => Some(fenced(EXAMPLE_PLAYGROUND)),
        "example_agent_commit" => Some(fenced(EXAMPLE_AGENT_COMMIT)),
        "example_dashboard" => Some(fenced(EXAMPLE_DASHBOARD)),
        "example_workflow" => Some(fenced(EXAMPLE_WORKFLOW)),
        "example_todo_extension" => Some(fenced(EXAMPLE_TODO_EXTENSION)),
        "example_hocus_complete" => Some(fenced(EXAMPLE_HOCUS_COMPLETE)),
        "example_todo_pane" => Some(fenced(EXAMPLE_TODO_PANE)),
        "example_hocus_todos" => Some(fenced(EXAMPLE_HOCUS_TODOS)),
        "example_hocus_notes" => Some(fenced(EXAMPLE_HOCUS_NOTES)),
        sdk if SDK_SECTIONS.contains(&sdk) => sdk_part(sdk).map(fenced),
        _ => None,
    }
}

fn fenced(source: &str) -> String {
    format!("```ts\n{}\n```", source.trim_end())
}

/// Answer a `plugin_docs` call.
///
/// An unknown section lists the valid ones rather than erroring, so the model corrects itself on
/// the next turn instead of retrying the same wrong name.
pub fn docs(requested: Option<&str>) -> String {
    let requested = requested.map(str::trim).filter(|s| !s.is_empty());
    let name = requested.unwrap_or("index");
    match section(name) {
        Some(text) => text,
        None => format!(
            "no such section {name:?}; the sections are: {}",
            SECTIONS.join(", ")
        ),
    }
}

/// Every section a query runs over: all but the index, in `SECTIONS` order, or `only` that one.
/// Each is the text a `section=` fetch answers with, so a hit's line number is the line a page
/// read reads.
fn corpus(only: Option<&str>) -> Vec<(&'static str, String)> {
    SECTIONS
        .iter()
        .copied()
        .filter(|name| *name != "index")
        .filter(|name| only.is_none_or(|only| only == *name))
        .filter_map(|name| section(name).map(|text| (name, text)))
        .collect()
}

/// Answer a `plugin_docs query=`: `section:LINE:text` hits over every section, or over
/// `section` alone. The cheapest grain the docs come in - a spelling is found for a few lines
/// rather than for the 13 KB of the part it sits in.
pub fn search(section: Option<&str>, search: &Search) -> String {
    let section = section.map(str::trim).filter(|s| !s.is_empty());
    if let Some(name) = section {
        if self::section(name).is_none() {
            return docs(Some(name));
        }
    }
    let corpus = corpus(section);
    let files: Vec<(&str, &str)> = corpus
        .iter()
        .map(|(name, text)| (*name, text.as_str()))
        .collect();
    match plugin_source::search(&files, search) {
        Ok(Some(hits)) => hits,
        Ok(None) => "no matches in the docs for that query; plugin_docs with no arguments lists \
                     the sections"
            .to_owned(),
        Err(error) => error,
    }
}

/// Answer a `plugin_docs section=X start_line=N`: one page of one section in `read_answer`'s
/// shape, its footer naming `plugin_docs section=X` as the call that reads on.
pub fn page(section: Option<&str>, start: usize, end: usize) -> String {
    let Some(name) = section.map(str::trim).filter(|s| !s.is_empty()) else {
        return "start_line needs a section: plugin_docs section=<name> start_line=N".to_owned();
    };
    match self::section(name) {
        Some(text) => tools::read_answer(&text, start, end, &format!("plugin_docs section={name}")),
        None => docs(Some(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_advertised_section_resolves_to_something() {
        for name in SECTIONS {
            let text = section(name)
                .unwrap_or_else(|| panic!("'{name}' is advertised but resolves to nothing"));
            assert!(!text.trim().is_empty(), "'{name}' is empty");
        }
    }

    /// 3.8 KB of quickstart used to be appended to the index unasked, on the first `plugin_docs`
    /// call of every plugin conversation - and, being a tool result, on every round after it.
    /// The bound sits just above the index as it stands and well under what the quickstart
    /// would add back.
    #[test]
    fn the_index_does_not_carry_the_quickstart() {
        let index = docs(None);
        assert!(!index.contains("## A plugin"), "{index}");
        assert!(index.contains("quickstart"), "but it still points at it");
        // The budget includes the declaration parts and worked examples.
        assert!(
            index.len() < 5_400,
            "the index is {} bytes and is paid for on every later turn",
            index.len()
        );
    }

    /// "Read only the relevant sections" is not advice a model can act on without knowing which
    /// ones are cheap: the range is under a kilobyte to thirteen.
    #[test]
    fn the_index_states_a_size_for_every_section() {
        let index = docs(None);
        for name in SECTIONS {
            if name == "index" {
                continue;
            }
            let line = index
                .lines()
                .find(|line| line.starts_with(&format!("- {name} ")))
                .unwrap_or_else(|| panic!("the index has no line for '{name}'"));
            assert!(line.contains("k):"), "{line}");
        }
    }

    /// The index is the one thing a plugin request always sees, and the loop it names is the
    /// whole of how a plugin comes to exist here. A model that reaches the index without meeting
    /// `plugin_write` goes looking for some other way to put the file on disk.
    #[test]
    fn the_index_states_the_authoring_loop() {
        let index = docs(None);
        for tool in [
            "plugin_write",
            "plugin_outline",
            "plugin_search",
            "plugin_edit",
            "plugin_reload",
            "plugin_log",
            "query=",
        ] {
            assert!(index.contains(tool), "the index does not name '{tool}'");
        }
        assert!(
            index.contains("Never ask another agent to write a plugin"),
            "the index does not rule out delegating the write"
        );
    }

    /// A name listed twice is a line printed twice in the index and a duplicate in the schema's
    /// `enum`, which is how `files` sat there unnoticed.
    #[test]
    fn no_section_is_listed_twice() {
        let mut seen = std::collections::BTreeSet::new();
        for name in SECTIONS {
            assert!(seen.insert(name), "'{name}' is listed twice");
        }
    }

    #[test]
    fn the_index_names_every_other_section() {
        let index = docs(None);
        for name in SECTIONS {
            if name == "index" {
                continue;
            }
            assert!(index.contains(name), "the index does not mention '{name}'");
        }
    }

    #[test]
    fn an_unknown_section_lists_the_real_ones() {
        let answer = docs(Some("nonsense"));
        assert!(answer.contains("no such section"), "{answer}");
        assert!(answer.contains("quickstart"), "{answer}");
    }

    /// The declarations are cut at markers this file names; a marker moved or dropped would
    /// silently fold two parts into one and breach the cap, or serve an empty part.
    #[test]
    fn sdk_declarations_are_cut_at_every_marker() {
        assert_eq!(
            SDK.matches("// ---- section: ").count(),
            SDK_SECTIONS.len(),
            "every marker in sdk.d.ts is a section, and every section has one"
        );
        assert!(SDK.starts_with(&sdk_marker(SDK_SECTIONS[0])));
        let mut whole = String::new();
        for name in SDK_SECTIONS {
            let part = sdk_part(name).unwrap_or_else(|| panic!("no marker for {name}"));
            assert!(part.lines().count() > 5, "{name} is empty");
            whole.push_str(part);
        }
        assert_eq!(whole, SDK, "the parts are the whole file, in order");
    }

    /// Nothing may outgrow the tool-output cap, which would cut a section mid-sentence.
    #[test]
    fn no_section_is_anywhere_near_the_tool_output_cap() {
        for name in SECTIONS {
            let len = section(name).unwrap().len();
            assert!(
                len < MAX_DOCS_OUTPUT,
                "'{name}' is {len} bytes, which is too much to send on every later turn"
            );
        }
    }

    fn query(text: &str) -> Search<'_> {
        Search {
            query: text,
            case_insensitive: false,
            context: 0,
            max_hits: plugin_source::SEARCH_HITS,
        }
    }

    /// A query is the cheapest grain the docs come in: a few lines that say where a spelling is,
    /// rather than the 13 KB section it is in. So it has to stay far under the cap - and every
    /// hit has to name the line a page of its section would read, or the two grains disagree.
    #[test]
    fn a_docs_query_answers_section_line_hits_and_stays_far_under_the_cap() {
        let hits = search(None, &query("RunCommandResult"));
        assert!(hits.contains("sdk_core:"), "{hits}");
        // Keep repeated matches well below the output cap.
        assert!(
            hits.len() < MAX_DOCS_OUTPUT / 3,
            "{} bytes is not a cheap grain",
            hits.len()
        );
        for hit in hits
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('…'))
        {
            let mut parts = hit.splitn(3, ':');
            let name = parts.next().unwrap();
            let line: usize = parts
                .next()
                .unwrap()
                .parse()
                .unwrap_or_else(|_| panic!("{hit}"));
            let text = parts.next().unwrap();
            assert_eq!(
                section(name).unwrap().lines().nth(line - 1),
                Some(text),
                "{hit}"
            );
        }
    }

    #[test]
    fn a_docs_query_confined_to_a_section_searches_only_it() {
        let hits = search(Some("sdk_core"), &query("Event"));
        assert!(
            hits.lines().all(|line| line.starts_with("sdk_core:")
                || line.is_empty()
                || line.starts_with('…')),
            "{hits}"
        );
        assert!(search(Some("nonsense"), &query("x")).contains("no such section"));
        let none = search(None, &query("qzxv_nothing_like_this"));
        assert!(none.contains("no matches in the docs"), "{none}");
    }

    /// A page of a section names the section as the way to read on - `plugin_read`'s footer
    /// shape, with the call that actually fetches it.
    #[test]
    fn a_docs_page_names_the_next_call_by_section() {
        let first = page(Some("sdk_commands"), 1, 20);
        assert!(first.starts_with("lines 1-20 of "), "{first}");
        assert!(
            first.contains("(plugin_docs section=sdk_commands start_line=21)"),
            "{first}"
        );
        let refused = page(None, 1, 20);
        assert!(
            refused.contains("plugin_docs section=<name> start_line=N"),
            "{refused}"
        );
        assert!(page(Some("nonsense"), 1, 20).contains("no such section"));
    }

    #[test]
    fn an_invalid_docs_regex_is_answered_with_the_regex_error() {
        let bad = search(None, &query("("));
        assert!(bad.contains("regex parse error"), "{bad}");
    }
}

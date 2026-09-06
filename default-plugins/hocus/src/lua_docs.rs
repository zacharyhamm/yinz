//! The Lua plugin reference, served to the model as a tool.
//!
//! Everything here is compiled in: the guide and the worked examples are checked-in files, and
//! `include_str!` means the tool cannot go stale against them or fail because a doc was moved.
//!
//! It is sectioned for cost rather than for size. Every section fits inside `MAX_TOOL_OUTPUT`
//! several times over, but a tool result stays in the transcript and is re-sent on every later
//! turn of the conversation — so handing the guide over whole would tax every message that
//! followed it. Asking for a section is how the model pays only for what it needs.
//!
//! No number is quoted here, because the last one rotted: the guide had doubled since. The rule
//! lives in `no_section_is_anywhere_near_the_tool_output_cap` instead, which fails when a
//! section grows past what a tool call should cost. The two `##` sections that did outgrow it -
//! `Commands` at 16 KB and `Hooks` at 12 KB - are served as their `###` children, so a model
//! that wants the permission table does not also buy the pane-workflow guide.

use crate::plugin_source::{self, Search};
use crate::tools;

/// How much of a `plugin_docs` answer reaches the model.
///
/// Well above every section as they stand - the largest is a prelude part at ~13 KB - and that
/// is the point: this is not the mechanism, `SECTIONS` is. It is the backstop that keeps a
/// guide section which grows past what a tool call should cost from silently landing whole in
/// a transcript that then re-sends it on every later turn.
pub const MAX_DOCS_OUTPUT: usize = 16_000;

/// The guide, verbatim.
const GUIDE: &str = include_str!("../../../docs/LUA_PLUGINS.md");
const EXAMPLE_CLOCK: &str = include_str!("../../../example-plugins/clock.lua");
const EXAMPLE_PLAYGROUND: &str = include_str!("../../../example-plugins/playground.lua");
const EXAMPLE_AGENT_COMMIT: &str = include_str!("../../../example-plugins/agent-commit.lua");
const EXAMPLE_DASHBOARD: &str = include_str!("../../../example-plugins/dashboard.lua");
const EXAMPLE_WORKFLOW: &str = include_str!("../../../example-plugins/workflow.lua");
const EXAMPLE_TODO_EXTENSION: &str = include_str!("../../../example-plugins/todo-extension.lua");
const EXAMPLE_HOCUS_COMPLETE: &str = include_str!("../../../example-plugins/hocus-complete.lua");
const EXAMPLE_TODO_PANE: &str = include_str!("../../../example-plugins/todo-pane.lua");
const EXAMPLE_HOCUS_TODOS: &str = include_str!("../../../example-plugins/hocus-todos.lua");
const EXAMPLE_HOCUS_NOTES: &str = include_str!("../../../example-plugins/hocus-notes.lua");
const EXAMPLE_CAPABILITIES: &str = include_str!("../../../example-plugins/capabilities.lua");
const EXAMPLE_AGENT_HARNESS: &str = include_str!("../../../example-plugins/agent-harness.lua");
/// The Lua half of the API, verbatim: the argument sugar and the named wrappers live here, so
/// a model that has read it does not have to guess how a command is spelled. Served in nine
/// parts, cut at its own section markers, so that none is more than a tool call should cost.
const PRELUDE: &str = include_str!("../../../zellij-server/src/plugins/lua/prelude.lua");
const PRELUDE_CUTS: [&str; 8] = [
    "\n-- Host commands ",
    "\n-- Tasks ",
    "\n-- The awaitables.",
    "\n-- Agents ",
    "\n-- Plugin-to-plugin calls ",
    "\n-- Data ",
    "\n-- Rendering ",
    "\n-- Pane workflows ",
];

/// Part `index` of the prelude: before the first cut, between the two, or after the second.
fn prelude_part(index: usize) -> &'static str {
    let starts: Vec<usize> = PRELUDE_CUTS
        .iter()
        .filter_map(|cut| PRELUDE.find(cut).map(|at| at + 1))
        .collect();
    let start = index
        .checked_sub(1)
        .and_then(|i| starts.get(i).copied())
        .unwrap_or(0);
    let end = starts.get(index).copied().unwrap_or(PRELUDE.len());
    &PRELUDE[start..end]
}

/// The section names the schema advertises, and the only ones `section` accepts.
///
/// Each guide section is named by the `##` heading it is cut at rather than by a byte offset, so
/// editing the guide cannot silently start returning the wrong text — it fails a test instead.
pub const SECTIONS: [&str; 48] = [
    "index",
    "quickstart",
    "contract",
    "events",
    "commands",
    "commands_workflows",
    "permissions",
    "tasks",
    "agents",
    "plugin_calls",
    "hocus_completions",
    "rendering",
    "modules",
    "data",
    "hocus_data",
    "queues",
    "hocus_extensions",
    "hooks",
    "hooks_workflow",
    "hooks_headless",
    "hooks_context_menu",
    "files",
    "errors",
    "restrictions",
    "limits",
    "example_forms",
    "prelude_widgets",
    "example_clock",
    "example_capabilities",
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
    "prelude",
    "prelude_commands",
    "prelude_tasks",
    "prelude_awaitables",
    "prelude_agents",
    "prelude_rpc",
    "prelude_data",
    "prelude_ui",
    "prelude_workflows",
];

/// The `##` heading each guide section is cut at. Examples are not in the guide and are absent.
fn heading(section: &str) -> Option<&'static str> {
    Some(match section {
        "quickstart" => "A plugin",
        "contract" => "The contract",
        "events" => "Events",
        "commands" | "commands_workflows" | "permissions" => "Commands",
        "tasks" => "Tasks",
        "agents" => "Agents",
        "plugin_calls" => "Plugin-to-plugin calls",
        "hocus_completions" => "Hocus completions",
        "rendering" => "Rendering",
        "modules" => "Modules",
        "data" => "Data",
        "hocus_data" => "Hocus data",
        "queues" => "Work queues",
        "hocus_extensions" => "Hocus extensions",
        "hooks" | "hooks_workflow" | "hooks_headless" | "hooks_context_menu" => "Hooks",
        "files" => "Files",
        "errors" => "Errors",
        "restrictions" => "What Lua plugins may not do",
        "limits" => "Current limits",
        _ => return None,
    })
}

/// How much of its `##` section a name resolves to.
///
/// Two of the guide's sections are far larger than the rest - `Commands` at ~16 KB and `Hooks`
/// at ~12 KB - and a model that wanted one paragraph of either paid for all of it, permanently.
/// So those two are served as their `###` children, and the historical names keep the part
/// everyone needs rather than becoming aliases for the whole thing again.
#[derive(Clone, Copy)]
enum Cut {
    /// The whole `##` section, `###` children and all. What every other section is.
    Whole,
    /// The section down to, but not including, the named `###` child.
    Until(&'static str),
    /// One named `###` child, on its own.
    Only(&'static str),
}

/// The `###` child a name is cut at, and whether it is the head or the child that is kept.
fn cut(section: &str) -> Cut {
    match section {
        // The preamble, the keybinding rules and the short "commands that answer" - what a
        // plugin that runs a command needs, and nothing about workflows or permissions.
        "commands" => Cut::Until("Pane workflows"),
        "commands_workflows" => Cut::Only("Pane workflows"),
        "permissions" => Cut::Only("Permissions"),
        "hooks" => Cut::Until("Project-owned Swamp workflows"),
        "hooks_workflow" => Cut::Only("Project-owned Swamp workflows"),
        "hooks_headless" => Cut::Only("Headless hooks"),
        "hooks_context_menu" => Cut::Only("The `context_menu` hook"),
        _ => Cut::Whole,
    }
}

/// One `## `-delimited section of the guide, heading included, cut down to `cut`.
///
/// Split at read time rather than kept as offsets: ten splits of a 10 KB string costs nothing,
/// and it means the guide is the single source of truth for where a section begins and ends.
/// The `###` split is the same trick one level down, which is why renaming a subheading fails a
/// test rather than silently returning the wrong text.
fn guide_section(heading: &str, cut: Cut) -> Option<String> {
    let chunk = GUIDE
        .split("\n## ")
        .skip(1)
        .find(|chunk| chunk.starts_with(heading))?;
    let text = match cut {
        Cut::Whole => format!("## {}", chunk.trim_end()),
        Cut::Until(sub) => {
            let at = chunk.find(&format!("\n### {sub}"))?;
            // The children left behind have to be nameable, or the model has no reason to
            // guess that `commands_agents` exists and simply does without what is in it.
            let rest: Vec<String> = subheadings(chunk)
                .iter()
                .filter_map(|name| section_for_sub(heading, name).map(|s| (s, *name)))
                .map(|(name, sub)| format!("- {name}: \"{sub}\""))
                .collect();
            format!(
                "## {}\n\nThe rest of \"{heading}\" is served separately, so it is not paid for \
                 unless it is asked for. Call plugin_docs with:\n{}",
                chunk[..at].trim_end(),
                rest.join("\n")
            )
        },
        Cut::Only(sub) => {
            let child = chunk
                .split("\n### ")
                .skip(1)
                .find(|child| child.starts_with(sub))?;
            // The parent heading rides along: a child on its own reads as a fragment without
            // it, and it is one line.
            format!("## {heading}\n\n### {}", child.trim_end())
        },
    };
    Some(text)
}

/// The `### ` headings inside one `##` chunk, in order.
fn subheadings(chunk: &str) -> Vec<&str> {
    chunk
        .split("\n### ")
        .skip(1)
        .filter_map(|child| child.lines().next())
        .collect()
}

/// The section name that serves one `###` child, if any does on its own.
fn section_for_sub(parent: &str, sub: &str) -> Option<&'static str> {
    SECTIONS
        .iter()
        .copied()
        .find(|name| heading(name) == Some(parent) && matches!(cut(name), Cut::Only(s) if s == sub))
}

/// The default section: what the rest hold, plus the complete minimal plugin, which is the one
/// piece of the guide worth having without asking.
fn index() -> String {
    let mut out = String::from(
        "Zellij Lua plugin reference. You write these yourself: `plugin_write` puts the complete \
         file in the user's plugin folder once it compiles (`plugin_check` is that compile \
         alone), and approving the write grants the permissions the source asks for. To change \
         a plugin that works, `plugin_outline` or \
         `plugin_search` finds the lines, `plugin_read` reads that range, `plugin_edit` \
         replaces it - never re-send a file from memory. `plugin_reload` loads it and answers \
         with its first render or the load error; `plugin_log` has what it logged. Never ask \
         another agent to write a plugin. `plugin_docs query=<regex>` searches every section \
         below (section:LINE:text); `section=` fetches one, or part of it with \
         `start_line`/`line_count`:\n\n",
    );
    for section in SECTIONS {
        let description = match section {
            "index" => continue,
            "quickstart" => "a complete minimal plugin (below)".to_owned(),
            "example_clock" => "example: the small one".to_owned(),
            "example_capabilities" => "example: most of what a plugin can do, in one pane".to_owned(),
            "example_forms" => "example: shared forms, confirmation and scrollable Markdown".to_owned(),
            "prelude_widgets" => "shared widget constructors and event wrappers".to_owned(),
            "example_agent_harness" => "example: an agent harness - spawn, prompt, check, hand failures back until green".to_owned(),
            "example_playground" => "example: every shape of command in use".to_owned(),
            "example_agent_commit" => "example: an agent_exit hook".to_owned(),
            "example_dashboard" => "example: a themed dashboard - zellij.ui, a timer, a command, keys and the mouse".to_owned(),
            "example_workflow" => "example: a multi-step command sequence in one pane (pane_workflow)".to_owned(),
            "example_todo_extension" => "example: a hocus extension owning a shared: data namespace - permissions, schema, registration, tools".to_owned(),
            "example_hocus_complete" => "example: a plugin's own model turn, looping on the tool calls it gets back".to_owned(),
            "example_todo_pane" => "example: the pane half of the same shared data - a watch, keys, and an input mode".to_owned(),
            "example_hocus_todos" => "example: reading hocus's own todo rows out of the shared data store".to_owned(),
            "example_hocus_notes" => "example: hocus's notes, tags and todo links".to_owned(),
            "prelude" => "the Lua side of the API itself: events, commands, files, and every named wrapper".to_owned(),
            "prelude_commands" => "its host-command wrappers and keybinding helpers".to_owned(),
            "prelude_tasks" => "zellij.task/await and the timers".to_owned(),
            "prelude_awaitables" => "the zellij.async awaitables".to_owned(),
            "prelude_agents" => "its zellij.agent part".to_owned(),
            "prelude_rpc" => "its zellij.rpc part".to_owned(),
            "prelude_data" => "its zellij.data part: the persistent store, with every helper".to_owned(),
            "prelude_ui" => "its zellij.text (widths) and zellij.ui (themed text, ribbons, tables, lists) parts".to_owned(),
            "prelude_workflows" => "the rest of it: pane_workflow and run_command_in_pane - command sequences in one pane".to_owned(),
            "commands" => "the guide's \"Commands\", less the two parts below".to_owned(),
            "tasks" => "zellij.task/await: straight-line code over commands, panes, agents and timers".to_owned(),
            "agents" => "spawning, observing and steering coding agents in panes".to_owned(),
            "plugin_calls" => "zellij.rpc: a call with an answer, between plugins".to_owned(),
            "hocus_completions" => "zellij.hocus.complete: a plugin's own model turn".to_owned(),
            "commands_workflows" => "pane_workflow: a command sequence in one pane".to_owned(),
            "permissions" => "every permission a plugin can request, and what each allows"
                .to_owned(),
            "hooks" => "the guide's \"Hooks\", and the agent_exit one".to_owned(),
            "hooks_workflow" => "project-owned Swamp launch, questions, evidence and recovery".to_owned(),
            "hooks_headless" => "hooks that run with no pane".to_owned(),
            "hooks_context_menu" => "the context_menu hook".to_owned(),
            other => match heading(other) {
                Some(heading) => format!("the guide's \"{heading}\""),
                None => continue,
            },
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
    out.push_str("\nStart with `quickstart` unless you already know the shape.\n");
    out
}

/// The text for a section, or `None` if nothing goes by that name.
pub fn section(name: &str) -> Option<String> {
    match name {
        "index" => Some(index()),
        "example_clock" => Some(fenced(EXAMPLE_CLOCK)),
        "example_capabilities" => Some(fenced(EXAMPLE_CAPABILITIES)),
        "example_forms" => Some(fenced(include_str!("../../../example-plugins/forms.lua"))),
        "prelude_widgets" => Some(fenced(include_str!(
            "../../../zellij-server/src/plugins/lua/ui.lua"
        ))),
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
        "prelude" => Some(fenced(prelude_part(0))),
        "prelude_commands" => Some(fenced(prelude_part(1))),
        "prelude_tasks" => Some(fenced(prelude_part(2))),
        "prelude_awaitables" => Some(fenced(prelude_part(3))),
        "prelude_agents" => Some(fenced(prelude_part(4))),
        "prelude_rpc" => Some(fenced(prelude_part(5))),
        "prelude_data" => Some(fenced(prelude_part(6))),
        "prelude_ui" => Some(fenced(prelude_part(7))),
        "prelude_workflows" => Some(fenced(prelude_part(8))),
        other => heading(other).and_then(|heading| guide_section(heading, cut(other))),
    }
}

fn fenced(source: &str) -> String {
    format!("```lua\n{}\n```", source.trim_end())
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
/// rather than for the 13 KB of the prelude part it sits in.
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

    /// Every `## ` heading in the guide, in order. Only the tests need this: it is how they
    /// check the section table against the file it names headings in.
    fn headings() -> Vec<&'static str> {
        GUIDE
            .split("\n## ")
            .skip(1)
            .filter_map(|chunk| chunk.lines().next())
            .collect()
    }

    #[test]
    fn every_advertised_section_resolves_to_something() {
        for name in SECTIONS {
            let text = section(name)
                .unwrap_or_else(|| panic!("'{name}' is advertised but resolves to nothing"));
            assert!(!text.trim().is_empty(), "'{name}' is empty");
        }
    }

    /// The coupling this file cannot enforce by construction: a section names a heading in a
    /// file it does not own, so renaming that heading must fail here rather than quietly turn
    /// `plugin_docs section=events` into "no such section".
    #[test]
    fn every_section_heading_still_exists_in_the_guide() {
        let headings = headings();
        for name in SECTIONS {
            let Some(heading) = heading(name) else {
                continue; // an example, which is not in the guide
            };
            assert!(
                headings.contains(&heading),
                "the guide has no '## {heading}' for section '{name}'; its headings are {headings:?}"
            );
        }
    }

    /// The other half of the same coupling: a section of the guide nothing can ask for is a
    /// section the model will never see.
    #[test]
    fn every_guide_heading_is_reachable_as_a_section() {
        for heading in headings() {
            assert!(
                SECTIONS
                    .iter()
                    .any(|name| super::heading(name) == Some(heading)),
                "the guide's '## {heading}' is not reachable through any section name"
            );
        }
    }

    #[test]
    fn a_section_is_cut_at_the_next_heading() {
        let events = section("events").unwrap();
        assert!(events.starts_with("## Events"), "{events}");
        assert!(
            !events.contains("\n## Commands"),
            "a section must stop at the next heading"
        );
        // The `###` subsections belong to the `##` they sit under and travel with it, unless
        // the `##` was too big to send whole - see `cut`.
        let commands = section("commands").unwrap();
        assert!(commands.contains("### Commands that answer"), "{commands}");
        assert!(
            !commands.contains("### Permissions"),
            "the permission table is its own section now: {commands}"
        );
    }

    /// The `###` half of the coupling `every_guide_heading_is_reachable_as_a_section` checks for
    /// `##`: a child of a section that is served in parts must be reachable, or it is a piece of
    /// the guide the model can never see. Children of a `Whole` section travel with their
    /// parent and need no name of their own.
    #[test]
    fn every_subheading_of_a_split_section_is_reachable() {
        for parent in ["Commands", "Hooks"] {
            let chunk = GUIDE
                .split("\n## ")
                .skip(1)
                .find(|chunk| chunk.starts_with(parent))
                .unwrap_or_else(|| panic!("the guide has no '## {parent}'"));
            let served = SECTIONS
                .iter()
                .filter(|name| heading(name) == Some(parent))
                .filter_map(|name| section(name))
                .collect::<Vec<_>>()
                .join("\n");
            for sub in subheadings(chunk) {
                assert!(
                    served.contains(&format!("### {sub}")),
                    "'### {sub}' is in '## {parent}' but no section serves it"
                );
            }
        }
    }

    /// A part left out of the section that used to hold it is a part the model has no reason to
    /// know exists. The head has to name what it no longer carries.
    #[test]
    fn a_split_section_names_the_parts_it_no_longer_carries() {
        let commands = section("commands").unwrap();
        for name in ["commands_workflows", "permissions"] {
            assert!(commands.contains(name), "{commands}");
        }
        let hooks = section("hooks").unwrap();
        for name in ["hooks_headless", "hooks_context_menu"] {
            assert!(hooks.contains(name), "{hooks}");
        }
    }

    /// 3.8 KB of quickstart used to be appended to the index unasked, on the first `plugin_docs`
    /// call of every plugin conversation - and, being a tool result, on every round after it.
    /// The bound sits just above the index as it stands - the orienting tools and the query
    /// grain cost it ~150 bytes - and well under what the quickstart would add back.
    #[test]
    fn the_index_does_not_carry_the_quickstart() {
        let index = docs(None);
        assert!(!index.contains("## A plugin"), "{index}");
        assert!(index.contains("quickstart"), "but it still points at it");
        // The budget grew with the tasks, agents, rpc and completion parts of the prelude and
        // the guide sections that go with them; it is still one short listing.
        assert!(
            index.len() < 4_100,
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

    /// The two sections that were split are the reason this file exists in its current shape.
    /// If either grows back past a few kilobytes the split has stopped working.
    #[test]
    fn the_split_sections_are_a_fraction_of_what_they_were() {
        for name in ["commands", "hooks"] {
            let len = section(name).unwrap().len();
            assert!(
                len < 6_000,
                "'{name}' is back to {len} bytes; it is served in parts for a reason"
            );
        }
    }

    #[test]
    fn an_unknown_section_lists_the_real_ones() {
        let answer = docs(Some("nonsense"));
        assert!(answer.contains("no such section"), "{answer}");
        assert!(answer.contains("quickstart"), "{answer}");
    }

    /// Nothing may outgrow the tool-output cap, which would cut a section mid-sentence.
    /// The guide's "Hocus data" is the schema a plugin author writes fetches against, and the
    /// failure it exists to prevent is exactly a guess at a name that does not exist. So it is
    /// checked against the definitions themselves rather than trusted to be kept up by hand: an
    /// entity or an attribute added to `store::shared_defs()` has to be written down here too.
    ///
    /// `import_marker` is the one exception, and is named as such below: it is bookkeeping for
    /// hocus's own migration off the old JSON files, and means nothing to a reader.
    #[test]
    fn the_hocus_data_section_names_every_shared_entity_and_attribute() {
        let text = section("hocus_data").expect("the section resolves");
        for def in hocus_proto::store::shared_defs() {
            if def.name == "import_marker" {
                continue;
            }
            assert!(
                text.contains(&format!("`{}`", def.name)),
                "the guide does not name the `{}` entity",
                def.name
            );
            for attr in &def.attrs {
                assert!(
                    text.contains(&format!("`{}`", attr.name)),
                    "the guide does not name `{}`.`{}`",
                    def.name,
                    attr.name
                );
            }
        }
    }

    /// And the other way: a name in the guide that no definition carries is a reader sent to
    /// fetch a column that is not there.
    #[test]
    fn the_hocus_data_section_invents_no_entity_names() {
        let text = section("hocus_data").expect("the section resolves");
        let declared: Vec<String> = hocus_proto::store::shared_defs()
            .into_iter()
            .map(|def| def.name)
            .collect();
        for line in text.lines().filter(|line| line.starts_with("**`")) {
            let name = line
                .trim_start_matches("**`")
                .split('`')
                .next()
                .unwrap()
                .to_owned();
            assert!(
                declared.contains(&name),
                "the guide describes `{name}`, which no shared definition declares"
            );
        }
    }

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

    /// The guide is the only place a plugin author meets these names, and hocus pre-grants what
    /// it finds in a source lexically: a permission spelled wrong is granted to nobody and
    /// raises nothing until the plugin runs. So every permission zellij has must be nameable
    /// from the guide, and a permission added later must fail here rather than be undocumented.
    #[test]
    fn the_guide_names_every_permission_a_plugin_can_ask_for() {
        let permissions = section("permissions").expect("the permission table's own section");
        for name in zellij_tile::plugin_authoring::all_permission_names() {
            assert!(
                permissions.contains(&format!("`{name}`")),
                "the guide's permission table has no `{name}`"
            );
        }
    }

    /// The two store rules that cost the most to learn at run time: a namespace that is not
    /// `shared:`-prefixed is refused, and a declared `created_at` fails the whole define. Both
    /// are enforced in `zellij_types::plugin_data`, and both have to be sayable from the guide.
    #[test]
    fn the_guide_states_the_data_store_rules_it_cannot_enforce() {
        let data = section("data").expect("the data section");
        assert!(
            data.contains(zellij_tile::plugin_authoring::SHARED_NAMESPACE_PREFIX),
            "the guide does not mention the shared namespace prefix"
        );
        for reserved in zellij_tile::plugin_authoring::RESERVED_ATTR_NAMES {
            assert!(
                data.contains(reserved),
                "the guide does not say `{reserved}` is reserved"
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
        let hits = search(None, &query("run_command"));
        assert!(hits.contains("prelude_commands:"), "{hits}");
        assert!(
            hits.len() < MAX_DOCS_OUTPUT / 4,
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
        let hits = search(Some("events"), &query("event"));
        assert!(
            hits.lines().all(|line| line.starts_with("events:")
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
        let first = page(Some("prelude_commands"), 1, 20);
        assert!(first.starts_with("lines 1-20 of "), "{first}");
        assert!(
            first.contains("(plugin_docs section=prelude_commands start_line=21)"),
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

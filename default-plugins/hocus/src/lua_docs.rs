//! The Lua plugin reference, served to the model as a tool.
//!
//! Everything here is compiled in: the guide and the worked examples are checked-in files, and
//! `include_str!` means the tool cannot go stale against them or fail because a doc was moved.
//!
//! It is sectioned for cost rather than for size. Every section fits inside `MAX_TOOL_OUTPUT`
//! several times over, but a tool result stays in the transcript and is re-sent on every later
//! turn of the conversation — so handing over the whole 26 KB once would tax every message that
//! followed it. Asking for a section is how the model pays only for what it needs.

/// The guide, verbatim.
const GUIDE: &str = include_str!("../../../docs/LUA_PLUGINS.md");
const EXAMPLE_CLOCK: &str = include_str!("../../../example-plugins/clock.lua");
const EXAMPLE_PLAYGROUND: &str = include_str!("../../../example-plugins/playground.lua");
const EXAMPLE_AGENT_COMMIT: &str = include_str!("../../../example-plugins/agent-commit.lua");
const EXAMPLE_DASHBOARD: &str = include_str!("../../../example-plugins/dashboard.lua");
const EXAMPLE_WORKFLOW: &str = include_str!("../../../example-plugins/workflow.lua");
const EXAMPLE_TODO_EXTENSION: &str = include_str!("../../../example-plugins/todo-extension.lua");
const EXAMPLE_TODO_PANE: &str = include_str!("../../../example-plugins/todo-pane.lua");
/// The Lua half of the API, verbatim: the argument sugar and the named wrappers live here, so
/// a model that has read it does not have to guess how a command is spelled. Served in five
/// parts, cut at its own section markers, so that none is more than a tool call should cost.
const PRELUDE: &str = include_str!("../../../zellij-server/src/plugins/lua/prelude.lua");
const PRELUDE_CUTS: [&str; 4] = [
    "\n-- Host commands ",
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
pub const SECTIONS: [&str; 27] = [
    "index",
    "quickstart",
    "contract",
    "events",
    "commands",
    "rendering",
    "modules",
    "data",
    "queues",
    "hocus_extensions",
    "hooks",
    "files",
    "errors",
    "restrictions",
    "limits",
    "example_clock",
    "example_playground",
    "example_agent_commit",
    "example_dashboard",
    "example_workflow",
    "example_todo_extension",
    "example_todo_pane",
    "prelude",
    "prelude_commands",
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
        "commands" => "Commands",
        "rendering" => "Rendering",
        "modules" => "Modules",
        "data" => "Data",
        "queues" => "Work queues",
        "hocus_extensions" => "Hocus extensions",
        "hooks" => "Hooks",
        "files" => "Files",
        "errors" => "Errors",
        "restrictions" => "What Lua plugins may not do",
        "limits" => "Current limits",
        _ => return None,
    })
}

/// One `## `-delimited section of the guide, heading included.
///
/// Split at read time rather than kept as offsets: ten splits of a 10 KB string costs nothing,
/// and it means the guide is the single source of truth for where a section begins and ends.
fn guide_section(heading: &str) -> Option<String> {
    GUIDE
        .split("\n## ")
        .skip(1)
        .find(|chunk| chunk.starts_with(heading))
        .map(|chunk| format!("## {}", chunk.trim_end()))
}

/// The default section: what the rest hold, plus the complete minimal plugin, which is the one
/// piece of the guide worth having without asking.
fn index() -> String {
    let mut out = String::from(
        "Zellij Lua plugin reference. You write these yourself: `plugin_write` puts the complete \
         file in the user's plugin folder after compiling it (`plugin_check` runs that compile \
         on its own, and both warn about what a compile cannot see), and approving that write \
         grants the permissions the source asks for; `plugin_edit` changes part of a plugin \
         that already works, which is what to reach for rather than re-sending a file from \
         memory; `plugin_reload` loads it and answers with its first render or the load error; \
         `plugin_log` has what it logged. Never ask another agent to write a plugin. Call \
         plugin_docs with a `section` for the rest:\n\n",
    );
    for section in SECTIONS {
        let description = match section {
            "index" => continue,
            "quickstart" => "a complete minimal plugin (below)".to_owned(),
            "example_clock" => "example: the small one".to_owned(),
            "example_playground" => "example: every shape of command in use".to_owned(),
            "example_agent_commit" => "example: an agent_exit hook".to_owned(),
            "example_dashboard" => "example: a themed dashboard - zellij.ui, a timer, a command, keys and the mouse".to_owned(),
            "example_workflow" => "example: a multi-step command sequence in one pane (pane_workflow)".to_owned(),
            "example_todo_extension" => "example: a hocus extension owning a shared: data namespace - permissions, schema, registration, tools".to_owned(),
            "example_todo_pane" => "example: the pane half of the same shared data - a watch, keys, and an input mode".to_owned(),
            "prelude" => "the Lua side of the API itself: events, commands, files, and every named wrapper".to_owned(),
            "prelude_commands" => "its host-command wrappers and keybinding helpers".to_owned(),
            "prelude_data" => "its zellij.data part: the persistent store, with every helper".to_owned(),
            "prelude_ui" => "its zellij.text (widths) and zellij.ui (themed text, ribbons, tables, lists) parts".to_owned(),
            "prelude_workflows" => "the rest of it: pane_workflow and run_command_in_pane - command sequences in one pane".to_owned(),
            other => match heading(other) {
                Some(heading) => format!("the guide's \"{heading}\""),
                None => continue,
            },
        };
        out.push_str(&format!("- {section}: {description}\n"));
    }
    if let Some(quickstart) = heading("quickstart").and_then(guide_section) {
        out.push('\n');
        out.push_str(&quickstart);
    }
    out
}

/// The text for a section, or `None` if nothing goes by that name.
pub fn section(name: &str) -> Option<String> {
    match name {
        "index" => Some(index()),
        "example_clock" => Some(fenced(EXAMPLE_CLOCK)),
        "example_playground" => Some(fenced(EXAMPLE_PLAYGROUND)),
        "example_agent_commit" => Some(fenced(EXAMPLE_AGENT_COMMIT)),
        "example_dashboard" => Some(fenced(EXAMPLE_DASHBOARD)),
        "example_workflow" => Some(fenced(EXAMPLE_WORKFLOW)),
        "example_todo_extension" => Some(fenced(EXAMPLE_TODO_EXTENSION)),
        "example_todo_pane" => Some(fenced(EXAMPLE_TODO_PANE)),
        "prelude" => Some(fenced(prelude_part(0))),
        "prelude_commands" => Some(fenced(prelude_part(1))),
        "prelude_data" => Some(fenced(prelude_part(2))),
        "prelude_ui" => Some(fenced(prelude_part(3))),
        "prelude_workflows" => Some(fenced(prelude_part(4))),
        other => heading(other).and_then(guide_section),
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
        // The `###` subsections belong to the `##` they sit under and travel with it.
        let commands = section("commands").unwrap();
        assert!(commands.contains("### Commands that answer"), "{commands}");
    }

    /// The index is the one thing a plugin request always sees, and the loop it names is the
    /// whole of how a plugin comes to exist here. A model that reaches the index without meeting
    /// `plugin_write` goes looking for some other way to put the file on disk.
    #[test]
    fn the_index_states_the_authoring_loop() {
        let index = docs(None);
        for tool in ["plugin_write", "plugin_edit", "plugin_reload", "plugin_log"] {
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

    /// Nothing may outgrow the tool-output cap, which would cut a section mid-sentence.
    #[test]
    fn no_section_is_anywhere_near_the_tool_output_cap() {
        for name in SECTIONS {
            let len = section(name).unwrap().len();
            assert!(
                len < 20_000,
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
        let permissions = section("commands").expect("the permissions live under Commands");
        for name in zellij_tile::plugin_authoring::all_permission_names() {
            assert!(
                permissions.contains(&format!("`{name}`")),
                "the guide's permission table has no `{name}`"
            );
        }
    }

    /// The two store rules that cost the most to learn at run time: a namespace that is not
    /// `shared:`-prefixed is refused, and a declared `created_at` fails the whole define. Both
    /// are enforced in `zellij_utils::plugin_data`, and both have to be sayable from the guide.
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
}

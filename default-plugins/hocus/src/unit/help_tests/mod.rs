use super::*;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use crate::extensions::Registry;
use crate::tools::ToolCall;

/// A registry holding one `weather_forecast` tool, the same fixture shape
/// `extensions_tests` builds, so the two files describe the same extension.
fn extension_registry(safety: &str) -> Registry {
    let discovered =
        BTreeMap::from([("weather".to_owned(), "/plugins/hocus/weather.ts".to_owned())]);
    let instances = BTreeMap::from([(
        zellij_tile::prelude::PluginId::new(7),
        ("weather".to_owned(), "/plugins/hocus/weather.ts".to_owned()),
    )]);
    let registration = json!({
        "version": crate::extensions::VERSION,
        "namespace": "weather",
        "path": "/plugins/hocus/weather.ts",
        "tools": [{
            "name": "forecast",
            "description": "Read the forecast.",
            "parameters": {
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"],
            },
            "safety": safety,
            "consequence": "asks a weather service about a city",
        }],
    })
    .to_string();
    let mut registry = Registry::default();
    registry
        .register(
            zellij_tile::prelude::PluginId::new(7),
            &registration,
            &discovered,
            &instances,
            &BTreeSet::new(),
        )
        .expect("the fixture registration is valid");
    registry
}

fn help(name: &str) -> String {
    lookup(name, &Registry::default()).unwrap_or_else(|| panic!("no help for {name}"))
}

// -- completeness -------------------------------------------------------

/// The whole point of `--help` is that it answers for anything you can type.
/// A tool added to `tools::specs` with no paragraph here would fall back to
/// its model-facing one-liner, which is exactly the state this replaces.
#[test]
fn every_tool_has_help() {
    let written: Vec<&str> = TOOL_HELP.iter().map(|(name, _)| *name).collect();
    for name in tools::names() {
        assert!(
            written.contains(&name.as_str()),
            "no help paragraph for {name}"
        );
    }
    for name in &written {
        assert!(
            tools::names().contains(&(*name).to_owned()),
            "{name} has help but is not a tool"
        );
    }
}

/// The plugin's own commands have no schema to fall back on, so a missing
/// entry here is a command with no help at all.
#[test]
fn every_local_command_has_help() {
    let written: Vec<&str> = COMMAND_HELP.iter().map(|(name, _)| *name).collect();
    for name in crate::LOCAL_COMMANDS {
        assert!(written.contains(&name), "no help for /{name}");
    }
    for name in &written {
        assert!(
            crate::LOCAL_COMMANDS.contains(name),
            "{name} has help but is not a command"
        );
    }
}

/// Help goes out as a `Message::notice`, which the transcript wraps rather
/// than renders as markdown, so any syntax would be shown literally. The
/// same constraint `present::commands_with` is held to.
#[test]
fn help_is_plain_text() {
    for name in tools::names().iter().chain(
        crate::LOCAL_COMMANDS
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
            .iter(),
    ) {
        let text = help(name);
        assert!(!text.contains('`'), "{name} help holds a backtick");
        assert!(!text.contains('#'), "{name} help holds a #");
        assert!(!text.contains('*'), "{name} help holds a *");
    }
}

// -- the rendering ------------------------------------------------------

/// The usage line is the shape a shell's is, because that is the shape the
/// reader already knows: required bare, optional in brackets.
#[test]
fn the_usage_line_marks_required_arguments() {
    let text = help("send_input");
    let usage = text.lines().next().unwrap();
    assert_eq!(
        usage,
        "/send_input is_plugin=<boolean> pane_id=<integer> [keys=<array of string>] \
         [text=<string>]"
    );
}

#[test]
fn schema_property_order_does_not_change_help_order() {
    let parameters: Value = serde_json::from_str(
        r#"{"properties":{"d":{"type":"string"},"c":{"type":"string"},"b":{"type":"string"},"a":{"type":"string"}},"required":["c","a"]}"#,
    ).unwrap();
    assert_eq!(
        usage_line("example", &parameters),
        "/example a=<string> c=<string> [b=<string>] [d=<string>]"
    );
    assert_eq!(
        argument_table(&parameters).unwrap(),
        "arguments\n  a  string  required\n  b  string\n  c  string  required\n  d  string"
    );
}

/// Eighteen bracketed pairs on one line is a paragraph of punctuation, and
/// the table under it names them all anyway.
#[test]
fn a_long_option_list_becomes_one_word() {
    assert_eq!(
        help("agent_spawn").lines().next().unwrap(),
        "/agent_spawn [options]"
    );
    // Still named while they fit, which is the whole point of the cap.
    assert_eq!(
        help("new_tab").lines().next().unwrap(),
        "/new_tab [cwd=<string>] [name=<string>]"
    );
    // The table is what the usage line defers to, so it must be whole.
    for key in [
        "worktree_dir",
        "reuse",
        "allow_outside_worktree",
        "permission_mode",
        "firecracker_image",
    ] {
        assert!(help("agent_spawn").contains(key), "{key} is named nowhere");
    }
}

/// A default recorded in the schema is the other thing the table says that no
/// prose should repeat: read from `specs()`, it cannot be wrong about the value
/// the tool actually uses.
#[test]
fn a_schema_description_is_shown_under_its_row() {
    let text = help("read_pane");
    assert!(
        text.contains("      Defaults to false; may be long."),
        "{text}"
    );
    // Meaning before the closed set, which is the order `argument_table` emits.
    let placement = help("warp_pane_here");
    let default = placement.find("Defaults to new_tab.").expect("the default");
    let values = placement.find("one of: new_tab").expect("the values");
    assert!(default < values, "{placement}");
}

/// The schema strings are written for the model and are allowed a backtick;
/// this file's answers are not. `help_is_plain_text` is the wider guard - this
/// names the one description that would fail it, so a rewrite of `plain` that
/// stops stripping is caught where the cause is.
#[test]
fn a_backtick_in_a_description_is_stripped() {
    assert_eq!(
        plain("Session id to resume, or `last`."),
        "Session id to resume, or last."
    );
    assert!(help("agent_spawn").contains("Session id to resume, or last."));
}

/// A closed set is the one thing the argument table can say that no prose
/// should have to repeat, so it has to actually appear.
#[test]
fn a_closed_set_is_listed() {
    let text = help("send_input");
    assert!(text.contains("one of: enter, tab, escape"), "{text}");
    assert!(text.contains("ctrl_z"), "{text}");
    assert!(
        help("warp_pane_here").contains("one of: new_tab, focused_tab, floating"),
        "{}",
        help("warp_pane_here")
    );
}

/// An empty table is worse than none: it says a tool takes arguments and
/// then names none.
#[test]
fn a_tool_with_no_arguments_has_no_argument_table() {
    let text = help("list_session");
    assert!(!text.contains("arguments"), "{text}");
    assert!(!text.contains("key=value"), "{text}");
    assert_eq!(text.lines().next().unwrap(), "/list_session");
}

/// The manual path skips the y/n on purpose. Someone who has only ever seen
/// the model's calls stop would not guess that, so the help says it.
#[test]
fn a_confirming_tool_says_so() {
    assert!(
        help("send_input").contains("approval"),
        "{}",
        help("send_input")
    );
    assert!(!help("list_session").contains("approval"));
    assert!(!help("close_pane").contains("approval"));
}

/// Four tools stop only in one shape. The generic sentence would say a bare
/// /new_pane stops, and it does not, so each of them says which shape.
#[test]
fn a_conditionally_confirming_tool_says_which_shape() {
    assert!(
        help("new_pane").contains("with no command opens a shell"),
        "{}",
        help("new_pane")
    );
    assert!(
        help("agent_spawn").contains("with no prompt waits"),
        "{}",
        help("agent_spawn")
    );
    assert!(
        help("schedule").contains("auto_confirm"),
        "{}",
        help("schedule")
    );
}

/// `confirms_line` and `tools::call_needs_confirmation` are two readings of
/// one fact. A tool that stops with no sentence would be a surprise, and a
/// sentence on a tool that never stops would be a lie.
#[test]
fn every_confirming_tool_has_a_sentence() {
    for name in tools::names() {
        let call = ToolCall {
            id: "x".to_owned(),
            name: name.clone(),
            // Enough to make each conditional tool take its stopping shape.
            arguments: r#"{"command":"ls","prompt":"go","auto_confirm":true}"#.to_owned(),
        };
        assert_eq!(
            tools::call_needs_confirmation(&call),
            confirms_line(&name).is_some(),
            "{name}"
        );
    }
}

/// A deferred family is a fact about the model's tool array, not about what
/// you may type, and the help has to say which.
#[test]
fn the_family_row_marks_a_deferred_family() {
    assert!(help("list_session").contains("Family: session."));
    assert!(
        help("plugin_write").contains("tools_load"),
        "{}",
        help("plugin_write")
    );
}

/// An extension tool has no paragraph here — nobody in this repository wrote
/// it — so it renders from the description its own registration carried.
#[test]
fn an_extension_tool_renders_from_its_registration() {
    let registry = extension_registry("confirm");
    let text = lookup("weather_forecast", &registry).expect("help for an extension tool");
    assert!(
        text.starts_with("/weather_forecast city=<string>"),
        "{text}"
    );
    assert!(text.contains("Read the forecast."), "{text}");
    assert!(text.contains("approval"), "{text}");
    assert!(text.contains("weather extension"), "{text}");
    assert!(!text.contains("Family:"), "{text}");
}

/// A read-only extension tool must not claim it stops for an approval it
/// never asks for.
#[test]
fn a_read_only_extension_tool_does_not_claim_a_confirmation() {
    let registry = extension_registry("read_only");
    let text = lookup("weather_forecast", &registry).unwrap();
    assert!(!text.contains("approval"), "{text}");
}

#[test]
fn a_name_nothing_answers_to_has_no_help() {
    assert!(lookup("nonesuch", &Registry::default()).is_none());
    // A skill is never looked up here; `run_manual` excludes them before it
    // asks, and this is the other half of that contract.
    assert!(lookup("bench", &Registry::default()).is_none());
}

// -- the request --------------------------------------------------------

/// `--help` is also an ordinary string a tool can be given, and two tests in
/// `tools_tests` already pass it as a search query. Only a bare flag is a
/// request for help.
#[test]
fn only_a_bare_help_flag_is_a_help_request() {
    assert_eq!(split_help_request("/send_input --help"), Some("send_input"));
    assert_eq!(
        split_help_request("  /send_input   -h  "),
        Some("send_input")
    );
    assert_eq!(split_help_request("/plan --help"), Some("plan"));
    assert_eq!(split_help_request("/search_files query=--help"), None);
    assert_eq!(split_help_request("/send_input text=--help"), None);
    assert_eq!(split_help_request("/send_input --help pane_id=3"), None);
    assert_eq!(split_help_request("/send_input"), None);
    assert_eq!(split_help_request("--help"), None);
    assert_eq!(split_help_request("/"), None);
}

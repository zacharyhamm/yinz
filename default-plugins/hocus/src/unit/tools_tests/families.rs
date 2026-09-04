use super::*;
use std::collections::BTreeSet;

fn loaded(names: &[&'static str]) -> BTreeSet<&'static str> {
    names.iter().copied().collect()
}

fn advertised(specs: &Value) -> Vec<String> {
    specs
        .as_array()
        .unwrap()
        .iter()
        .map(|spec| spec["function"]["name"].as_str().unwrap().to_owned())
        .collect()
}

/// The test the whole design rests on. A tool in no family is advertised to
/// everyone, which is merely wasteful; a tool in two would be filed under
/// whichever `family_of` reached first, which is the kind of thing that is
/// noticed months later. Neither is allowed to happen quietly.
#[test]
fn every_tool_is_in_exactly_one_family() {
    for name in names() {
        if name == LOAD_TOOL {
            // The loader is the one tool no family holds: it is how a family is
            // asked for, so it cannot be behind one.
            assert!(family_of(&name).is_none(), "{name} should be family-less");
            continue;
        }
        let matches: Vec<&str> = FAMILIES
            .iter()
            .filter(|family| family.tools.contains(&name.as_str()))
            .map(|family| family.name)
            .collect();
        assert_eq!(matches.len(), 1, "{name} is in families {matches:?}");
    }
}

/// The other direction: a family naming a tool that no longer exists would
/// silently stop disclosing anything, and the renamed tool would fall out of
/// every family into "advertised to everyone".
#[test]
fn every_family_names_real_tools() {
    let real: BTreeSet<String> = names().into_iter().collect();
    for family in FAMILIES {
        for tool in family.tools {
            assert!(real.contains(*tool), "{}: no such tool {tool}", family.name);
        }
    }
    assert!(
        FAMILIES.iter().any(|family| family.deferred),
        "a build with nothing deferred advertises everything and should fail here",
    );
}

/// Family names are what `tools_load`'s enum offers and what the log reports, so
/// two families cannot share one.
#[test]
fn family_names_are_unique() {
    let unique: BTreeSet<&str> = FAMILIES.iter().map(|family| family.name).collect();
    assert_eq!(unique.len(), FAMILIES.len());
}

/// The deferred families exist for dispatch too, under their own names, and the
/// two lists have to agree: a tool routed to `start_plugin_tool` but disclosed
/// with `skill` would be advertised at the wrong moment.
#[test]
fn deferred_families_match_the_dispatch_consts() {
    let tools_in = |name: &str| -> BTreeSet<&'static str> {
        FAMILIES
            .iter()
            .find(|family| family.name == name)
            .unwrap()
            .tools
            .iter()
            .copied()
            .collect()
    };
    assert_eq!(tools_in("plugin"), PLUGIN_TOOLS.into_iter().collect());
    assert_eq!(tools_in("slack"), SLACK_TOOLS.into_iter().collect());
    assert_eq!(tools_in("skill"), SKILL_TOOLS.into_iter().collect());
    assert_eq!(tools_in("linear"), LINEAR_TOOLS.into_iter().collect());
    // The always-on `code` family is `PROJECT_READ_TOOLS` under a different name,
    // plus `project_list`: that const is the dispatch route, not a family, and the
    // names differ so the `project` family is not confused with it. `project_list`
    // is disclosed with the readers whose "no active project" it clears, but it
    // runs through `run_local` rather than `start_project_read`, so it is in the
    // family without being in the const.
    let mut code: BTreeSet<&'static str> = PROJECT_READ_TOOLS.into_iter().collect();
    code.insert("project_list");
    assert_eq!(tools_in("code"), code);
}

#[test]
fn a_fresh_conversation_is_told_only_about_the_always_on_families() {
    let shown = advertised(&specs_for(&BTreeSet::new()));
    for family in FAMILIES {
        for tool in family.tools {
            let told = shown.iter().any(|name| name == tool);
            assert_eq!(told, !family.deferred, "{tool} in {}", family.name);
        }
    }
    assert!(shown.iter().any(|name| name == LOAD_TOOL));
}

/// `project_target` and `todo_target` answer "no active project; … call
/// project_list", and a backend that constrains generation to the advertised set
/// can only follow that if `project_list` is in it from the start. A family
/// shuffle that defers it again should fail here rather than in a conversation
/// that has no way out of the error it was just given.
#[test]
fn a_fresh_conversation_can_call_project_list() {
    let shown = advertised(&specs_for(&BTreeSet::new()));
    assert!(shown.iter().any(|name| name == "project_list"));
}

#[test]
fn loading_a_family_adds_exactly_its_tools() {
    let before = advertised(&specs_for(&BTreeSet::new()));
    let after = advertised(&specs_for(&loaded(&["plugin"])));
    let added: BTreeSet<&str> = after
        .iter()
        .filter(|name| !before.contains(name))
        .map(String::as_str)
        .collect();
    assert_eq!(added, PLUGIN_TOOLS.into_iter().collect());
}

#[test]
fn loading_every_family_advertises_the_whole_set() {
    let every: BTreeSet<&str> = deferred_families().into_iter().collect();
    assert_eq!(advertised(&specs_for(&every)), names());
}

// ---------------------------------------------------------------------------
// Deriving the loaded set from the transcript
// ---------------------------------------------------------------------------

fn families_of(calls: &[ToolCall]) -> BTreeSet<&'static str> {
    loaded_families(calls.iter())
}

#[test]
fn a_transcript_with_no_calls_has_loaded_nothing() {
    assert!(families_of(&[]).is_empty());
}

/// The unlock-on-call path, and the reason there is no round trip to pay: the
/// model names a tool it read about in the system prompt, `run_head` runs it
/// because dispatch never consults the advertised set, and the call sitting in
/// the transcript is what advertises the family from the next request on.
#[test]
fn calling_a_withheld_tool_loads_its_family() {
    let calls = [call("plugin_write", r#"{"name":"clock","source":"x"}"#)];
    assert_eq!(families_of(&calls), loaded(&["plugin"]));
}

#[test]
fn an_always_on_tool_loads_nothing() {
    let calls = [call("list_session", "{}"), call("todo_list", "{}")];
    assert!(families_of(&calls).is_empty());
}

#[test]
fn a_load_call_loads_the_family_it_names() {
    let calls = [call(LOAD_TOOL, r#"{"family":"slack"}"#)];
    assert_eq!(families_of(&calls), loaded(&["slack"]));
}

/// A load that was answered with an error loaded nothing, so it must leave
/// nothing behind here either — otherwise the model is told it failed and then
/// silently gets the tools anyway.
#[test]
fn a_load_of_something_that_is_not_a_deferred_family_loads_nothing() {
    for arguments in [
        r#"{"family":"nonsense"}"#,
        // Always-on: real, but not a family anyone loads.
        r#"{"family":"session"}"#,
        "{}",
        "not json",
    ] {
        let calls = [call(LOAD_TOOL, arguments)];
        assert!(families_of(&calls).is_empty(), "{arguments}");
    }
}

/// The two paths that read a family name have to agree, and the padding a model
/// leaves around a JSON string value is where they used to part: the loader
/// trimmed and answered "loaded", the derivation did not and advertised nothing,
/// so the model was told it had tools it never got. Asserted together, because
/// either half passing alone is exactly the bug.
#[test]
fn a_padded_family_name_loads_the_family_it_names() {
    for (arguments, padded) in [
        (r#"{"family":"slack "}"#, "slack "),
        (r#"{"family":" slack"}"#, " slack"),
        (r#"{"family":"\nslack\n"}"#, "\nslack\n"),
    ] {
        let calls = [call(LOAD_TOOL, arguments)];
        assert_eq!(families_of(&calls), loaded(&["slack"]), "{padded:?}");
        assert!(
            load_family(Some(padded), false).starts_with("slack loaded"),
            "{padded:?}",
        );
    }
}

#[test]
fn families_accumulate_across_a_conversation() {
    let calls = [
        call(LOAD_TOOL, r#"{"family":"agent"}"#),
        call("list_session", "{}"),
        call("plugin_read", r#"{"name":"clock"}"#),
        call("agent_list", "{}"),
    ];
    assert_eq!(families_of(&calls), loaded(&["agent", "plugin"]));
}

/// `Elision` rewrites the arguments of a `plugin_write` it has superseded, so the
/// derivation must not depend on them parsing — the name is what carries the
/// family, and elision never touches a name.
#[test]
fn an_elided_call_still_loads_its_family() {
    let calls = [call(
        "plugin_write",
        "{\"source\":\"[elided: superseded]\"}",
    )];
    assert_eq!(families_of(&calls), loaded(&["plugin"]));
}

// ---------------------------------------------------------------------------
// The loader's answer
// ---------------------------------------------------------------------------

#[test]
fn loading_a_family_answers_with_the_tools_it_brought() {
    let answer = load_family(Some("linear"), false);
    for tool in LINEAR_TOOLS {
        assert!(answer.contains(tool), "{answer}");
    }
}

/// In planning mode a family arrives half-withheld, and the answer has to say so:
/// a model that reads back the whole family picks the one tool the mode refuses
/// and spends a round being told no.
#[test]
fn a_load_in_planning_mode_lists_only_what_is_callable() {
    let answer = load_family(Some("plugin"), true);
    assert!(answer.contains("plugin_docs"), "{answer}");
    assert!(!answer.contains("plugin_write"), "{answer}");
    assert!(answer.contains("withholds"), "{answer}");
}

#[test]
fn a_family_that_only_writes_says_so_rather_than_listing_nothing() {
    // `schedule` and `unschedule` are both absent from READ_ONLY.
    let answer = load_family(Some("schedule"), true);
    assert!(answer.contains("planning mode"), "{answer}");
    assert!(!answer.contains("0 now available"), "{answer}");
}

#[test]
fn an_unknown_family_is_answered_with_the_ones_that_exist() {
    let answer = load_family(Some("nonsense"), false);
    assert!(answer.contains("no such tool family"), "{answer}");
    assert!(answer.contains("plugin"), "{answer}");
}

#[test]
fn loading_an_always_on_family_says_it_is_already_there() {
    let answer = load_family(Some("session"), false);
    assert!(answer.contains("already"), "{answer}");
}

#[test]
fn a_load_with_no_family_names_the_ones_that_exist() {
    for missing in [None, Some(""), Some("  ")] {
        let answer = load_family(missing, false);
        assert!(answer.contains("needs a family"), "{answer}");
    }
}

// ---------------------------------------------------------------------------
// The loader's own spec
// ---------------------------------------------------------------------------

/// Loading a family changes nothing, and planning mode has to be able to widen
/// itself — a plan about plugins is researched with the read-only half of the
/// `plugin` family, which is unreachable without this.
#[test]
fn the_loader_is_read_only() {
    assert!(READ_ONLY.contains(&LOAD_TOOL));
    assert!(advertised(&read_only_specs())
        .iter()
        .any(|n| n == LOAD_TOOL));
}

/// The enum and the index are both built from `FAMILIES`, so this is really
/// checking that a family added later cannot be offered without being explained.
#[test]
fn the_loader_offers_exactly_the_deferred_families() {
    let spec = specs()
        .as_array()
        .unwrap()
        .iter()
        .find(|spec| spec["function"]["name"] == LOAD_TOOL)
        .unwrap()
        .clone();
    let offered: Vec<String> = spec["function"]["parameters"]["properties"]["family"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(offered, deferred_families());
    assert_eq!(
        spec["function"]["parameters"]["required"],
        json!(["family"])
    );

    let description = spec["function"]["description"].as_str().unwrap();
    for family in FAMILIES.iter().filter(|family| family.deferred) {
        assert!(description.contains(family.name), "{description}");
        assert!(description.contains(family.summary), "{description}");
    }
    for family in FAMILIES.iter().filter(|family| !family.deferred) {
        assert!(!description.contains(family.summary), "{description}");
    }
}

// ---------------------------------------------------------------------------
// Planning mode and disclosure are independent
// ---------------------------------------------------------------------------

/// The two filters compose as an intersection, and both directions matter:
/// `plugin_docs` only reads but is withheld until the family is loaded, and
/// `plugin_write` stays withheld once it is.
#[test]
fn planning_mode_and_disclosure_are_both_required() {
    let unloaded = advertised(&read_only_specs_for(&BTreeSet::new()));
    assert!(!unloaded.iter().any(|name| name == "plugin_docs"));
    assert!(unloaded.iter().any(|name| name == "list_session"));

    let with_plugin = advertised(&read_only_specs_for(&loaded(&["plugin"])));
    assert!(with_plugin.iter().any(|name| name == "plugin_docs"));
    assert!(!with_plugin.iter().any(|name| name == "plugin_write"));
}

/// Planning mode is the smaller of the two sets whatever is loaded, so a chat
/// that has loaded everything still cannot act from within a plan.
#[test]
fn a_fully_loaded_plan_is_still_only_the_read_only_tools() {
    let every: BTreeSet<&str> = deferred_families().into_iter().collect();
    assert_eq!(
        advertised(&read_only_specs_for(&every)),
        advertised(&read_only_specs()),
    );
}

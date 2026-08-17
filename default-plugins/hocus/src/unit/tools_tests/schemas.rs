use super::*;

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
            &[
                "swap_panes",
                "set_floating_pane_geometry",
                "set_global_pane_placement",
            ][..],
        ),
        ("set_global_pane_placement", &["no tab"][..]),
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

/// The same reasoning as the warp placements: the four edges are the whole set, and a
/// fifth would only be found out about at dispatch. Only the pane is required — the
/// rest of a placement is inherited from where the pane already sits, which is what
/// lets "make it wider" be one argument rather than three.
#[test]
fn the_global_placement_schema_closes_the_edges_and_requires_only_the_pane() {
    let specs = specs();
    let spec = specs
        .as_array()
        .unwrap()
        .iter()
        .find(|spec| spec["function"]["name"] == "set_global_pane_placement")
        .expect("set_global_pane_placement should be advertised");
    let parameters = &spec["function"]["parameters"];
    assert_eq!(
        parameters["properties"]["edge"]["enum"],
        json!(["left", "right", "top", "bottom"])
    );
    assert_eq!(parameters["required"], json!(["pane_id", "is_plugin"]));
    for field in ["size", "length"] {
        assert_eq!(
            parameters["properties"][field]["type"], "string",
            "{field} is written like \"20\" or \"20%\", so it is a string"
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

/// The tab warps take no `placement`: a warped tab always lands as its own
/// new tab in the receiving session, so there is nothing to choose.
#[test]
fn the_warp_tab_schemas_require_a_destination_and_take_no_placement() {
    let specs = specs();
    for (name, required) in [
        ("warp_tab", json!(["to_session"])),
        ("warp_tab_here", json!(["from_session", "tab_id"])),
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
        assert_eq!(
            spec["function"]["parameters"]["properties"]["placement"],
            Value::Null,
            "{name} should not offer a placement",
        );
    }
}

#[test]
fn names_lists_the_advertised_tools() {
    let names = names();
    assert_eq!(names.len(), specs().as_array().unwrap().len());
    assert!(names.contains(&"new_pane".to_owned()));
    assert!(names.contains(&"new_remote_pane".to_owned()));
}

/// Only the destination is required. A bare host means that machine's login
/// shell, exactly as a bare `new_pane` means this machine's — and as
/// `--remote-host` with no command does on the CLI.
#[test]
fn new_remote_pane_requires_only_a_host() {
    let specs = specs();
    let parameters = specs
        .as_array()
        .unwrap()
        .iter()
        .find(|spec| spec["function"]["name"] == "new_remote_pane")
        .unwrap()["function"]["parameters"]
        .clone();
    assert_eq!(parameters["required"], json!(["remote_host"]));
    // The model only learns that omitting it is allowed if the description says
    // so; `new_pane` sets the wording.
    assert_eq!(
        parameters["properties"]["command"]["description"],
        json!("Omit for that machine's login shell.")
    );
}

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
        // Nothing is required at the schema either — `agent` and `command` are
        // alternatives, which JSON Schema's `required` cannot say and dispatch can.
        ("agent_spawn", Value::Null),
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

/// The three history tools carry the same filter block, because they take it from
/// one function. Pinned so that a filter added to one cannot quietly miss the
/// others — the failure that would cause is a `since` the model passes to
/// `agent_history_search` and that is silently ignored.
#[test]
fn every_agent_history_tool_offers_the_same_filter() {
    let specs = specs();
    let properties = |name: &str| {
        specs
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["function"]["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not advertised"))["function"]["parameters"]
            ["properties"]
            .clone()
    };
    for name in ["agent_runs", "agent_history_search"] {
        let properties = properties(name);
        for filter in ["kind", "project", "this_session", "since", "active"] {
            assert!(
                !properties[filter].is_null(),
                "{name} is missing the {filter} filter"
            );
        }
    }
}

/// `agent_history_search` needs a query and nothing else. The other two take
/// alternatives JSON Schema cannot express — `run_id` *or* `pane_id` — so they
/// require nothing at the schema and dispatch says which is missing, exactly as
/// `agent_spawn` does with `agent` and `command`.
#[test]
fn the_agent_history_tools_require_only_what_they_cannot_default() {
    let specs = specs();
    for (name, required) in [
        ("agent_runs", Value::Null),
        ("agent_run", Value::Null),
        ("agent_history_search", json!(["query"])),
    ] {
        let parameters = specs
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["function"]["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not advertised"))["function"]["parameters"]
            .clone();
        assert_eq!(parameters["required"], required, "{name}");
    }
}

/// They read rows another process wrote and change nothing, so planning mode
/// gets them. The generic tests cover that the names are real and unconfirmed;
/// this one pins that they are on the list at all — and, since they sit in the
/// deferred `agent` family, that a plan which loads it is actually offered them.
#[test]
fn the_agent_history_tools_are_available_when_planning() {
    let listed = advertised(&read_only_specs_for(&loaded(&["agent"])));
    for name in ["agent_runs", "agent_run", "agent_history_search"] {
        assert!(
            listed.contains(&name.to_owned()),
            "{name} cannot be planned with"
        );
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
        (
            "warp_tab_here",
            &["another session", "list_other_sessions", "current session"][..],
        ),
        ("schedule", &["unschedule"][..]),
        ("slack_read", &["backfill"][..]),
        ("slack_post", &["immediately"][..]),
        ("agent_send", &["listed agent"][..]),
        ("find_files", &["active project"][..]),
        ("search_files", &["active project"][..]),
        ("read_file", &["active project"][..]),
        ("git_log", &["Active project"][..]),
        ("git_show", &["Active project", "git_log"][..]),
        ("github_read", &["Active project", "GET only"][..]),
        ("close_pane", &["process"][..]),
        // The routing that was got wrong: a plugin is authored here, not handed to a spawned
        // coding agent, and each step of the loop names the next. "You write these yourself,
        // never by spawning an agent" used to be a fourth copy here; it is in the system
        // prompt, in the `plugin_docs` index and cued from `agent_spawn`, and this description
        // is on every request, so the copy went. What has to stay is the next step - and, since
        // this is the tool a whole-file rewrite goes through, the cheaper tool beside it.
        ("plugin_write", &["plugin_reload", "plugin_edit"][..]),
        (
            "plugin_edit",
            &["old_string", "not written at all", "edits"][..],
        ),
        (
            "plugin_read",
            &["start_line", "before you edit", "plugin_outline"][..],
        ),
        // The orienting tools: a hit's text is what an edit is built from, so the description
        // has to say which part of a hit to copy; an outline exists so the read after it can
        // be a range; and a docs query is the grain to try before buying a section.
        ("plugin_search", &["old_string", "never the prefix"][..]),
        ("plugin_outline", &["plugin_read", "range"][..]),
        ("plugin_docs", &["query", "start_line"][..]),
        ("plugin_reload", &["plugin_log"][..]),
        ("agent_spawn", &["plugin_write"][..]),
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
    for name in ["warp_pane_here", "adopt_orphaned_pane"] {
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
        here["function"]["parameters"]["properties"]["is_plugin"]["type"],
        json!("boolean"),
    );
}

/// The tab warp takes no `placement`: a warped tab always lands as its own
/// new tab in the receiving session, so there is nothing to choose.
#[test]
fn the_warp_tab_schema_requires_a_destination_and_takes_no_placement() {
    let specs = specs();
    let name = "warp_tab_here";
    let required = json!(["from_session", "tab_id"]);
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

/// The model only sends what the schema names. `new_pane` read both Firecracker
/// fields before it advertised either, which made them fields nothing ever set.
#[test]
fn new_pane_advertises_its_firecracker_fields() {
    let specs = specs();
    let parameters = specs
        .as_array()
        .unwrap()
        .iter()
        .find(|spec| spec["function"]["name"] == "new_pane")
        .unwrap()["function"]["parameters"]
        .clone();
    let properties = parameters["properties"].as_object().unwrap();
    for field in ["firecracker_vm", "firecracker_image"] {
        assert!(
            properties.contains_key(field),
            "new_pane does not list {field}"
        );
        assert_eq!(properties[field]["type"], json!("string"), "{field}");
    }
    assert!(properties["firecracker_image"]["description"]
        .as_str()
        .unwrap()
        .contains("firecracker_vm"));
}

#[test]
fn agent_spawn_advertises_an_automatic_firecracker_switch() {
    let specs = specs();
    let parameters = specs
        .as_array()
        .unwrap()
        .iter()
        .find(|spec| spec["function"]["name"] == "agent_spawn")
        .unwrap()["function"]["parameters"]
        .clone();
    assert_eq!(
        parameters["properties"]["firecracker"]["type"],
        json!("boolean")
    );
}

/// The Linear tools are advertised whether or not a key is configured, the same
/// as `web_search` and the Slack tools — "not configured" is answered as a tool
/// result the model can read, rather than by withholding the tool.
#[test]
fn the_linear_tools_are_always_advertised() {
    let names = crate::tools::names();
    for name in LINEAR_TOOLS {
        assert!(names.iter().any(|n| n == name), "{name} is not advertised");
    }
}

/// A free-string state would let the model send a workflow *name* ("In
/// Progress") where Linear wants a type, and get an empty list with no
/// explanation.
#[test]
fn linear_issues_closes_the_state_enum() {
    let specs = specs();
    let spec = specs
        .as_array()
        .unwrap()
        .iter()
        .find(|spec| spec["function"]["name"] == "linear_issues")
        .expect("linear_issues is advertised");
    let states = spec["function"]["parameters"]["properties"]["state"]["enum"]
        .as_array()
        .expect("state is a closed enum");
    assert_eq!(states.len(), crate::linear::STATES.len());
    for state in crate::linear::STATES {
        assert!(states.iter().any(|s| s == state), "{state} is missing");
    }
}

/// The agents you may ask for are a closed set, and it is the *same* closed set the
/// New agent modal offers — both read `launch::OFFERED`. An open string here would
/// let the model invent `"claude-code"` and only find out when the shell reported no
/// such program, after a worktree had already been created for it.
#[test]
fn agent_spawn_offers_exactly_the_agents_the_new_agent_modal_does() {
    let specs = specs();
    let spawn = specs
        .as_array()
        .unwrap()
        .iter()
        .find(|spec| spec["function"]["name"] == "agent_spawn")
        .unwrap();
    assert_eq!(
        spawn["function"]["parameters"]["properties"]["agent"]["enum"],
        json!(launch::OFFERED),
    );
}

// -- the read-only set --------------------------------------------------

/// The allowlist is the whole of planning mode's enforcement, so a name in it
/// that no longer exists is a tool silently dropped from the mode rather than a
/// compile error. This is the test that catches a rename.
#[test]
fn every_read_only_tool_is_a_real_tool() {
    let names = names();
    for tool in READ_ONLY {
        assert!(names.contains(&(*tool).to_owned()), "no such tool: {tool}");
    }
    // Counted rather than compared as sets, which is what catches the one fault
    // the loop above cannot: a name written into the allowlist twice.
    assert_eq!(
        advertised(&read_only_specs_for(&every_family())).len(),
        READ_ONLY.len(),
        "the filtered specs and the allowlist disagree",
    );
}

/// The two lists answer different questions — "changes nothing" and "changes
/// something badly enough to stop for a y/n" — and the first has to be strictly
/// inside the complement of the second. Pinned because the tempting refactor is
/// to derive one from the other, which would quietly let planning mode open
/// panes.
#[test]
fn nothing_read_only_is_ever_confirmed() {
    for tool in READ_ONLY {
        assert!(!needs_confirmation(tool), "{tool} both reads and confirms");
    }
}

/// A sample of what must stay out, one per family, so the exclusions are pinned
/// rather than left to whoever last edited the list.
#[test]
fn the_tools_that_act_are_withheld_from_planning() {
    // Everything loaded, so nothing here is absent merely for want of a family.
    let listed = advertised(&read_only_specs_for(&every_family()));
    for tool in [
        "close_pane",
        "new_pane",
        "focus",
        "scroll_pane",
        "send_input",
        "agent_spawn",
        "agent_register",
        "project_assign",
        "project_open",
        "todo_add",
        "note_add",
        "note_delete",
        "schedule",
        "unschedule",
        "slack_post",
        "slack_backfill",
        "plugin_write",
        "plugin_reload",
    ] {
        assert!(!READ_ONLY.contains(&tool), "{tool} is not read-only");
        assert!(
            !listed.contains(&tool.to_owned()),
            "{tool} is advertised to a plan"
        );
    }
}

/// The VM tools name a VM with `vm`, never with `name`.
///
/// `name` in this schema is a pane's, a project's and a skill's. A field that means
/// two things is a field the model fills in wrongly, and the failure would be a tool
/// call that silently acts on nothing.
#[test]
fn the_vm_tools_address_a_vm_by_vm_and_not_by_name() {
    let specs = specs();
    let specs = specs.as_array().unwrap();
    let spec = |name: &str| {
        specs
            .iter()
            .find(|spec| spec["function"]["name"] == name)
            .unwrap_or_else(|| panic!("no {name} tool"))
    };

    // The listing takes nothing: it is machine-wide, and there is no VM to name yet.
    let parameters = &spec("vm_list")["function"]["parameters"];
    assert_eq!(parameters["properties"], json!({}));
    assert!(parameters.get("required").is_none());

    for name in ["vm_changes", "vm_sync", "vm_close"] {
        let parameters = &spec(name)["function"]["parameters"];
        assert_eq!(parameters["required"], json!(["vm"]), "{name}");
        assert_eq!(parameters["properties"]["vm"]["type"], "string", "{name}");
        // Present but optional: the ordinary call names only the VM.
        assert_eq!(
            parameters["properties"]["vm_session"]["type"], "string",
            "{name}"
        );
        assert!(
            parameters["properties"].get("name").is_none(),
            "{name} advertises `name`, which means something else in every other schema"
        );
        assert!(parameters["properties"].get("session").is_none(), "{name}");
    }
}

// -- the orienting plugin tools ----------------------------------------------

/// The orienting tools ask for little - a query, or a name. `plugin_docs` keeps its closed
/// section enum, so a name outside it is refused with the list, and gains the query and the
/// page beside it; `plugin_edit` admits a batch beside the single form, so only the name is
/// required and the exactly-one rule is `Args::edit_batch`'s.
#[test]
fn the_orienting_tools_and_the_batch_edit_have_the_schemas_the_prompt_promises() {
    let specs = specs();
    let spec = |name: &str| {
        specs
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["function"]["name"] == name)
            .unwrap_or_else(|| panic!("no {name}"))["function"]["parameters"]
            .clone()
    };
    let search = spec("plugin_search");
    assert_eq!(search["required"], json!(["query"]));
    for key in [
        "query",
        "name",
        "case_insensitive",
        "context",
        "max_results",
    ] {
        assert!(
            search["properties"].get(key).is_some(),
            "plugin_search.{key}"
        );
    }
    let outline = spec("plugin_outline");
    assert_eq!(outline["required"], json!(["name"]));
    let docs = spec("plugin_docs");
    assert_eq!(
        docs["properties"]["section"]["enum"],
        json!(plugin_docs::SECTIONS.as_slice())
    );
    for key in [
        "query",
        "start_line",
        "line_count",
        "context",
        "case_insensitive",
    ] {
        assert!(docs["properties"].get(key).is_some(), "plugin_docs.{key}");
    }
    assert!(docs.get("required").is_none());
    let edit = spec("plugin_edit");
    assert_eq!(edit["required"], json!(["name"]));
    assert_eq!(
        edit["properties"]["edits"]["items"]["required"],
        json!(["old_string", "new_string"])
    );
    assert_eq!(
        edit["properties"]["edits"]["items"]["properties"]["replace_all"]["type"],
        "boolean"
    );
}

/// A plan about a plugin is researched with the tools that only read, and finding is reading.
#[test]
fn the_orienting_plugin_tools_are_available_when_planning() {
    for tool in ["plugin_search", "plugin_outline"] {
        assert!(READ_ONLY.contains(&tool), "{tool} is not read-only");
    }
    assert!(!READ_ONLY.contains(&"plugin_edit"));
    let listed = advertised(&read_only_specs_for(&loaded(&["plugin"])));
    assert!(
        listed.iter().any(|name| name == "plugin_search"),
        "{listed:?}"
    );
    assert!(
        listed.iter().any(|name| name == "plugin_outline"),
        "{listed:?}"
    );
    assert!(
        !listed.iter().any(|name| name == "plugin_edit"),
        "{listed:?}"
    );
}

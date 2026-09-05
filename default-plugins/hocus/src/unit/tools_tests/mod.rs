use super::*;

fn argv(name: &str, arguments: &str) -> Result<Vec<String>, String> {
    slack_argv(name, &Args::parse(arguments).unwrap())
}

fn project() -> Project {
    Project {
        name: "hocus".to_owned(),
        root: "/work/hocus".to_owned(),
        firecracker_image: None,
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

fn call(name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: "1".into(),
        name: name.into(),
        arguments: arguments.into(),
    }
}

fn positioned_pane(id: u32, x: usize, y: usize, columns: usize, rows: usize) -> PanePosition {
    PanePosition {
        id: PaneId::Terminal(id),
        tab_position: Some(0),
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

/// Two tabs in a session that has closed a tab: positions {0, 1}, ids {2, 3}. Every
/// id here is invalid as a position and vice versa, so a check that confuses the
/// two cannot pass by accident.
fn diverged_session() -> SessionTabs {
    SessionTabs {
        tabs: vec![2, 3],
        panes: vec![PaneId::Terminal(2), PaneId::Terminal(4)],
    }
}

/// The families a conversation has asked for.
fn loaded(names: &[&'static str]) -> BTreeSet<&'static str> {
    names.iter().copied().collect()
}

/// Every deferred family loaded — the widest any advertisement gets, and the
/// only state in which a spec array and a hand-written list of tool names can be
/// compared name for name.
fn every_family() -> BTreeSet<&'static str> {
    deferred_families().into_iter().collect()
}

/// The tool names a spec array offers, in the order it offers them.
fn advertised(specs: &Value) -> Vec<String> {
    specs
        .as_array()
        .unwrap()
        .iter()
        .map(|spec| spec["function"]["name"].as_str().unwrap().to_owned())
        .collect()
}

mod args;
mod confirmation;
mod families;
mod file_tools;
mod listing;
mod planners;
mod plugin_tools;
mod repository_tools;
mod schemas;
mod send_input;
mod slack;
mod tab_moves;
mod typed_calls;
mod vm;

#[test]
fn project_config_write_is_validated_and_shown_before_confirmation() {
    let call = call(
        "project_config_write",
        r#"{"source":"agent_exit { ci \"cargo\" \"x\" \"ci\" \"check\"; }","expected":null}"#,
    );
    assert!(call_needs_confirmation(&call));
    assert!(uncompilable_write(&call).is_none());
    assert!(confirmation_detail(&call).join("\n").contains("cargo"));
    assert!(consequence(&call).unwrap().contains("stale"));
    let invalid = super::tools_tests::call(
        "project_config_write",
        r#"{"source":"agent_exit { ci; }","expected":null}"#,
    );
    assert!(uncompilable_write(&invalid).unwrap().contains("invalid ci"));
    assert!(READ_ONLY.contains(&"project_config_read"));
    assert!(!READ_ONLY.contains(&"project_config_write"));
}

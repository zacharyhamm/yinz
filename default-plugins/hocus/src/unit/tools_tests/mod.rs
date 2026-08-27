use super::*;

fn argv(name: &str, arguments: &str) -> Result<Vec<String>, String> {
    slack_argv(name, &Args::parse(arguments).unwrap())
}

fn project() -> Project {
    Project {
        name: "hocus".to_owned(),
        root: "/work/hocus".to_owned(),
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

mod args;
mod confirmation;
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

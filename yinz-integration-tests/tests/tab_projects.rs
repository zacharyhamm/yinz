#![cfg(unix)]

//! The tab bar is the one place the project name is actually for.
//!
//! Both ends are unit-tested — agent works out the label from the agents in a
//! tab, and the server turns labels into names — but they are in different
//! processes with a protobuf between them, and neither test can say the name
//! made it onto the bar. Only a real session with a real grid says that.

use yinz_cli::args::CliAction;
use yinz_integration_tests::test_env::{write_git_repository, write_git_worktree};
use yinz_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

/// agent, the daemon that finds the agents, and the store that turns the
/// checkouts they are in into projects. All three are needed: without the store
/// there is a checkout but no project, and so nothing to name a tab after.
const AGENT_PROJECT_CONFIG: &str = r#"plugins {
  agent location="yinz:agents" {
    shared true
  }
  agents location="yinz:agents" {
    shared true
  }
  agent-service location="yinz:agent-service"
  project-store location="yinz:project-store"
}
load_plugins clear-defaults=true {
  "agent-service"
  "project-store"
}
"#;

fn mutate_store(yinz: &TestSession, op: &str, args: serde_json::Value) {
    let (code, output) = yinz.run_cli_action_reporting(CliAction::Pipe {
        focus_plugin: false,
        name: Some("store_mutate".into()),
        payload: Some(
            serde_json::json!({"op": op, "args": args, "structured_reply": true}).to_string(),
        ),
        args: None,
        plugin: None,
        plugin_configuration: None,
        configuration_json: None,
        force_launch_plugin: false,
        skip_plugin_cache: false,
        floating_plugin: None,
        in_place_plugin: None,
        plugin_cwd: None,
        plugin_title: None,
    });
    assert_eq!(code, 0, "{output}");
    let reply: serde_json::Value = serde_json::from_str(&output).expect("mutation reply");
    assert_eq!(reply["ok"], true, "{op}: {output}");
}

fn wait_for_tab(yinz: &TestSession, name: &str, absent: &[&str]) {
    yinz.wait_until(&format!("the tab shows {name}"), |screen| {
        screen
            .tab_bar_line()
            .is_some_and(|bar| bar.contains(name) && absent.iter().all(|old| !bar.contains(old)))
    });
}

#[test]
fn ordinary_checkout_panes_follow_program_osc_and_manual_titles() {
    let repository = write_git_repository("titles", "topic");
    let layout = format!(
        r#"layout {{
        pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
        pane command="/bin/zsh" cwd="{}"
        pane
    }}"#,
        repository.display()
    );
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(AGENT_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let shell = yinz.expect_pty_spawn();
    let second = yinz.expect_pty_spawn();
    shell.output(b"$ ");
    yinz.wait_until("shell checkout and program reach its frame", |screen| {
        screen
            .lines()
            .iter()
            .skip(1)
            .any(|line| line.contains("titles:topic: zsh"))
    });
    shell.run_foreground(&["/usr/bin/vim", "main.rs"]);
    yinz.wait_until(
        "foreground executable reaches the frame without arguments",
        |screen| {
            screen
                .lines()
                .iter()
                .skip(1)
                .any(|line| line.contains("titles:topic: vim") && !line.contains("main.rs"))
        },
    );
    shell.return_to_shell();
    yinz.wait_until("shell fallback returns to the frame", |screen| {
        screen
            .lines()
            .iter()
            .skip(1)
            .any(|line| line.contains("titles:topic: zsh"))
    });
    shell.output(b"\x1b]2;editing main.rs\x07");
    yinz.wait_until("OSC title reaches the frame", |screen| {
        screen
            .lines()
            .iter()
            .skip(1)
            .any(|line| line.contains("titles:topic: editing main.rs"))
    });
    assert_eq!(
        yinz.run_cli_action(CliAction::RenamePane {
            name: "manual".into(),
            pane_id: Some(shell.terminal_id().to_string()),
        }),
        0
    );
    yinz.wait_until("manual title reaches the frame", |screen| {
        screen
            .lines()
            .iter()
            .skip(1)
            .any(|line| line.contains("titles:topic: manual"))
    });
    assert_eq!(
        yinz.run_cli_action(CliAction::ClosePane {
            pane_id: Some(second.terminal_id().to_string()),
        }),
        0
    );
    wait_for_tab(&yinz, "titles:topic: manual", &[]);
    shell.run_foreground(&["/usr/bin/vim", "other.rs"]);
    assert_eq!(
        yinz.run_cli_action(CliAction::RenamePane {
            name: String::new(),
            pane_id: Some(shell.terminal_id().to_string()),
        }),
        0
    );
    wait_for_tab(&yinz, "titles:topic: editing main.rs", &["manual"]);
    assert_eq!(
        yinz.run_cli_action(CliAction::RenameTab {
            name: "my tab".into(),
            tab_id: None
        }),
        0
    );
    shell.output(b"\x1b]2;changed OSC\x07");
    wait_for_tab(&yinz, "my tab", &["changed OSC"]);
}

#[test]
fn agent_names_follow_moves_to_existing_and_new_tabs() {
    use yinz_types::input::actions::{hook_report_actions, Action};

    let repository = write_git_repository("moves", "main")
        .canonicalize()
        .unwrap();
    let worktree = write_git_worktree(&repository, "topic", "topic");
    let other = write_git_repository("other", "main");
    let layout = format!(
        r#"layout {{
        default_tab_template {{
            pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
            children
        }}
        tab focus=true {{
            pane command="/bin/zsh" cwd="{root}"
            pane command="/bin/zsh" cwd="{root}" focus=true
        }}
        tab {{ pane command="/bin/zsh" cwd="{other}"; }}
    }}"#,
        root = repository.display(),
        other = other.display(),
    );
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(AGENT_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let source = yinz.expect_pty_spawn();
    let agent = yinz.expect_pty_spawn();
    let destination = yinz.expect_pty_spawn();
    source.output(b"SOURCE");
    agent.run_foreground(&["codex"]);
    agent.output(b"MOVING_AGENT");
    destination.output(b"DESTINATION");
    mutate_store(
        &yinz,
        "project_remember",
        serde_json::json!({"name": "moves", "root": repository}),
    );
    let pane_id = yinz_types::data::PaneId::Terminal(agent.terminal_id());
    let report = |prompt: &str| {
        // The agent's worktree differs from its shell CWD, as it does after a checkout change.
        let raw = serde_json::json!({
            "hook_event_name": "UserPromptSubmit", "session_id": "moving-agent",
            "cwd": worktree, "prompt": prompt,
        })
        .to_string();
        let hook = serde_json::from_str(&raw).unwrap();
        let (code, output) =
            yinz.run_actions_reporting(hook_report_actions("codex", pane_id, &hook, raw));
        assert_eq!(code, 0, "{output}");
    };
    report("Before move");
    wait_for_tab(&yinz, "moves:main + topic", &[]);
    yinz.wait_until(
        "the agent event names the pane from its worktree",
        |screen| screen.contains("moves:topic: Before move"),
    );

    let (code, output) = yinz.run_actions_reporting(vec![Action::BreakPaneRight]);
    assert_eq!(code, 0, "{output}");
    wait_for_tab(&yinz, "moves:topic + other:main", &["main + topic"]);
    yinz.wait_until("the existing tab contains both panes", |screen| {
        screen.contains("MOVING_AGENT") && screen.contains("DESTINATION")
    });
    report("After existing move");
    yinz.wait_until("later events still name the moved pane", |screen| {
        screen.contains("moves:topic: After existing move")
    });

    let (code, output) = yinz.run_actions_reporting(vec![Action::BreakPane]);
    assert_eq!(code, 0, "{output}");
    wait_for_tab(
        &yinz,
        "moves:topic: After existing move",
        &["moves:topic + other:main"],
    );
    report("After new move");
    wait_for_tab(
        &yinz,
        "moves:topic: After new move",
        &["After existing move"],
    );
    let screen = yinz.wait_until("the new tab retains the moved terminal", |screen| {
        screen.contains("MOVING_AGENT") && !screen.contains("DESTINATION")
    });
    let bar = screen.tab_bar_line().unwrap();
    assert!(
        bar.contains("moves:main") && bar.contains("other:main"),
        "{bar}"
    );
}

#[test]
fn a_tab_is_named_after_the_project_its_agent_is_in() {
    let repository = write_git_repository("tab-project", "trunk");
    // The tab bar has to be in the layout: `clear-defaults` above took it away
    // along with everything else, and it is the thing under test. agent floats below it
    // (`y=1`): an overlay at the top-left row would cover the very line the test reads.
    let layout = format!(
        r#"layout {{
    pane size=1 borderless=true {{
        plugin location="yinz:tab-bar"
    }}
    floating_panes {{
        pane x=0 y=1 width="50%" height="90%" {{
            plugin location="agents" {{
                overlay true
            }}
        }}
    }}
    pane cwd="{}"
}}
"#,
        repository.display()
    );
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(AGENT_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let agent = yinz.expect_pty_spawn();
    agent.run_foreground(&["claude"]);
    agent.output(b"$ ");

    // The whole chain: the daemon probes the pane's folder, the store makes a
    // project of it, agent labels the tab, and the server puts the label on the
    // bar in place of the "Tab #1" the tab was born with.
    let screen = yinz.wait_until("the tab is named after the project", |grid_snapshot| {
        grid_snapshot
            .tab_bar_line()
            .is_some_and(|tab_bar| tab_bar.contains("tab-project"))
    });
    let tab_bar = screen.tab_bar_line().map(str::to_owned).unwrap_or_default();
    assert!(
        !tab_bar.contains("Tab #1"),
        "the project replaced the default name rather than sitting beside it:\n{screen}"
    );
}

#[test]
fn exited_agent_panel_groups_follow_project_rename_and_merge() {
    let source = write_git_repository("panel-source", "main")
        .canonicalize()
        .expect("source checkout");
    let target = write_git_repository("panel-target", "main")
        .canonicalize()
        .expect("target checkout");
    for repository in [&source, &target] {
        assert!(std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://example.com/panel/repo.git"
            ])
            .current_dir(repository)
            .status()
            .expect("add origin")
            .success());
    }
    let layout = format!(
        r#"layout {{
            tab name="manual tab" {{
                pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
                pane command="claude" cwd="{}"
                pane command="claude" cwd="{}"
                floating_panes {{
                    pane x=0 y=1 width="90%" height="90%" {{
                        plugin location="agents" {{ overlay true; }}
                    }}
                }}
            }}
        }}"#,
        source.display(),
        target.display()
    );
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(AGENT_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let agent = yinz.expect_pty_spawn();
    let other_agent = yinz.expect_pty_spawn();
    for (name, root) in [("panel-source", &source), ("panel-target", &target)] {
        mutate_store(
            &yinz,
            "project_remember",
            serde_json::json!({"name": name, "root": root}),
        );
    }
    yinz.wait_until("the open panel groups the agent", |screen| {
        screen.contains("panel-source")
            && screen.contains("panel-target")
            && screen.contains("claude")
    });
    assert_eq!(
        yinz.run_cli_action(CliAction::ClosePane {
            pane_id: Some(other_agent.terminal_id().to_string()),
        }),
        0
    );
    assert_eq!(
        yinz.run_cli_action(CliAction::RenameTab {
            name: "manual tab".into(),
            tab_id: None,
        }),
        0
    );
    wait_for_tab(&yinz, "manual tab", &[]);
    agent.exit(Some(0));
    yinz.wait_until("the open panel groups the exited agent", |screen| {
        screen.contains("panel-source") && screen.contains("claude") && screen.contains("held")
    });
    // The exited agent and manual tab name leave no pane activity to refresh the group.
    mutate_store(
        &yinz,
        "project_rename",
        serde_json::json!({"name": "panel-source", "new_name": "panel-renamed"}),
    );
    yinz.wait_until("the open panel shows the renamed group", |screen| {
        screen.contains("panel-renamed")
            && !screen.contains("panel-source")
            && screen.contains("held")
    });
    mutate_store(
        &yinz,
        "project_merge",
        serde_json::json!({"source": "panel-renamed", "target": "panel-target"}),
    );
    yinz.wait_until("the open panel shows the merged group", |screen| {
        screen.contains("panel-target")
            && !screen.contains("panel-renamed")
            && screen.contains("held")
    });
    wait_for_tab(&yinz, "manual tab", &[]);
    yinz.quit();
}

#[test]
fn a_lone_pane_title_takes_priority_over_its_project() {
    let folder = tempfile::tempdir().expect("shell folder");
    let layout = format!(
        r#"layout {{
    pane size=1 borderless=true {{
        plugin location="yinz:tab-bar"
    }}
    pane cwd="{}"
}}
"#,
        folder.path().display()
    );
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(AGENT_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(b"$ ");
    yinz.wait_until("the shell tab has its default name", |screen| {
        screen
            .tab_bar_line()
            .is_some_and(|bar| bar.contains("Tab #1"))
    });

    mutate_store(
        &yinz,
        "project_remember",
        serde_json::json!({
            "name": "quiet-shell-project", "root": folder.path(),
        }),
    );
    // No focus change or further shell output: the background plugins must publish it.
    yinz.wait_until(
        "the quiet shell tab takes the newly known project",
        |screen| {
            screen
                .tab_bar_line()
                .is_some_and(|bar| bar.contains("quiet-shell-project"))
        },
    );
    assert_eq!(
        yinz.run_cli_action(CliAction::RenamePane {
            name: "quiet-command".into(),
            pane_id: Some(shell.terminal_id().to_string()),
        }),
        0
    );
    wait_for_tab(&yinz, "quiet-command", &["quiet-shell-project"]);
    assert_eq!(
        yinz.run_cli_action(CliAction::RenameTab {
            name: "my-explicit-name".into(),
            tab_id: None,
        }),
        0
    );
    wait_for_tab(&yinz, "my-explicit-name", &["quiet-command"]);
    mutate_store(
        &yinz,
        "project_forget",
        serde_json::json!({"name": "quiet-shell-project"}),
    );
    mutate_store(
        &yinz,
        "project_remember",
        serde_json::json!({
            "name": "renamed-project", "root": folder.path(),
        }),
    );
    assert_eq!(
        yinz.run_cli_action(CliAction::UndoRenameTab { tab_id: None }),
        0
    );
    wait_for_tab(
        &yinz,
        "quiet-command",
        &["my-explicit-name", "renamed-project"],
    );
    assert_eq!(
        yinz.run_cli_action(CliAction::RenameTab {
            name: "another-name".into(),
            tab_id: None,
        }),
        0
    );
    wait_for_tab(&yinz, "another-name", &["quiet-command"]);
    assert_eq!(
        yinz.run_cli_action(CliAction::RenameTab {
            name: String::new(),
            tab_id: None,
        }),
        0
    );
    wait_for_tab(&yinz, "quiet-command", &["another-name"]);
    assert_eq!(
        yinz.run_cli_action(CliAction::RenamePane {
            name: String::new(),
            pane_id: Some(shell.terminal_id().to_string()),
        }),
        0
    );
    wait_for_tab(&yinz, "renamed-project", &["quiet-command"]);
    mutate_store(
        &yinz,
        "project_forget",
        serde_json::json!({"name": "renamed-project"}),
    );
    wait_for_tab(&yinz, "Tab #1", &["renamed-project"]);
}

#[test]
fn tab_projects_follow_cwd_changes_and_pane_closure_without_agent() {
    let a = write_git_repository("project-alpha", "main");
    let b = write_git_repository("project-beta", "main");
    let unknown = tempfile::tempdir().unwrap();
    let layout = format!(
        r#"layout {{
        pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
        pane cwd="{}"
        pane cwd="{}"
    }}"#,
        a.display(),
        unknown.path().display()
    );
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(AGENT_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let first = yinz.expect_pty_spawn();
    let second = yinz.expect_pty_spawn();
    wait_for_tab(&yinz, "project-alpha", &["project-beta"]);

    let worktree = a.join("topic-tree");
    let status = std::process::Command::new("git")
        .current_dir(&a)
        .args(["worktree", "add", "-b", "topic"])
        .arg(&worktree)
        .status()
        .expect("create worktree");
    assert!(status.success());
    second.output(format!("\x1b]7;file://localhost{}\x07", worktree.display()).as_bytes());
    wait_for_tab(&yinz, "project-alpha:main + topic", &[]);
    let status = std::process::Command::new("git")
        .current_dir(&worktree)
        .args(["checkout", "-b", "updated"])
        .status()
        .expect("change branch");
    assert!(status.success());
    // Refresh explicitly instead of waiting for the minute-long Git polling interval.
    assert_eq!(
        yinz.run_cli_action(CliAction::Pipe {
            focus_plugin: false,
            name: Some("agent_refresh".into()),
            payload: None,
            args: None,
            plugin: None,
            plugin_configuration: None,
            configuration_json: None,
            force_launch_plugin: false,
            skip_plugin_cache: false,
            floating_plugin: None,
            in_place_plugin: None,
            plugin_cwd: None,
            plugin_title: None,
        }),
        0
    );
    wait_for_tab(&yinz, "project-alpha:main + updated", &["topic"]);

    // Only the CWD notification, with no prompt output or focus change to trigger a redraw.
    second.output(format!("\x1b]7;file://localhost{}\x07", b.display()).as_bytes());
    wait_for_tab(&yinz, "project-alpha:main + project-beta:main", &[]);
    assert_eq!(
        yinz.run_cli_action(CliAction::ClosePane {
            pane_id: Some(first.terminal_id().to_string()),
        }),
        0
    );
    wait_for_tab(&yinz, "project-beta", &["project-alpha"]);
    second.output(format!("\x1b]7;file://localhost{}\x07", unknown.path().display()).as_bytes());
    wait_for_tab(&yinz, "Tab #1", &["project-beta"]);
}

#[test]
fn hidden_panes_keep_naming_the_tab_and_follow_cwd_changes() {
    let a = write_git_repository("hidden-alpha", "main");
    let b = write_git_repository("editor-beta", "main");
    let unknown = tempfile::tempdir().unwrap();
    let layout = format!(
        r#"layout {{
        pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
        pane cwd="{}"
    }}"#,
        a.display()
    );
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "{AGENT_PROJECT_CONFIG}\nscrollback_editor \"/usr/bin/true\""
        ))
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(b"SCROLLBACK_SOURCE");
    yinz.wait_until("scrollback source is available", |screen| {
        screen.contains("SCROLLBACK_SOURCE")
    });
    wait_for_tab(&yinz, "hidden-alpha", &[]);
    assert_eq!(
        yinz.run_cli_action(CliAction::EditScrollback {
            pane_id: Some(shell.terminal_id().to_string()),
            ansi: false,
        }),
        0
    );
    let editor = yinz.expect_pty_spawn();
    wait_for_tab(&yinz, "EDITING SCROLLBACK", &["hidden-alpha"]);
    assert_eq!(
        yinz.run_cli_action(CliAction::RenamePane {
            name: String::new(),
            pane_id: Some(editor.terminal_id().to_string()),
        }),
        0
    );
    wait_for_tab(
        &yinz,
        &format!("[{}] hidden-alpha", editor.terminal_id()),
        &[],
    );
    editor.output(format!("\x1b]7;file://localhost{}\x07", b.display()).as_bytes());
    wait_for_tab(&yinz, "editor-beta:main", &["hidden-alpha"]);
    shell.output(format!("\x1b]7;file://localhost{}\x07", unknown.path().display()).as_bytes());
    wait_for_tab(&yinz, "editor-beta", &["hidden-alpha"]);
    editor.exit(Some(0));
    wait_for_tab(&yinz, "Tab #1", &["editor-beta", "hidden-alpha"]);
}

#[test]
fn arrangement_cli_and_tab_binding_keep_both_running_terminals() {
    let repository = write_git_repository("arrangement-project", "main");
    let layout = format!(
        r#"layout {{
        tab {{
            pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
            pane cwd="{}"
        }}
        tab focus=true {{
            pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
            pane cwd="{}"
        }}
    }}"#,
        repository.display(),
        repository.display()
    );
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(AGENT_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let first = yinz.expect_pty_spawn();
    let second = yinz.expect_pty_spawn();
    first.output(b"ARRANGE_FIRST");
    second.output(b"ARRANGE_SECOND");
    yinz.wait_until("the store identifies the project", |screen| {
        screen.contains("arrangement-project")
    });
    assert_eq!(
        yinz.run_cli_action(yinz_cli::args::CliAction::ArrangePanesByProject),
        0
    );
    yinz.wait_until("the project terminals share one tab", |screen| {
        screen.contains("ARRANGE_FIRST") && screen.contains("ARRANGE_SECOND")
    });
    yinz.send_stdin(&[keys::ctrl('t'), keys::key('a')].concat());
    first.output(b"_STILL_RUNNING");
    second.output(b"_STILL_RUNNING");
    yinz.wait_until("both processes still render after the binding", |screen| {
        screen.contains("ARRANGE_FIRST_STILL_RUNNING")
            && screen.contains("ARRANGE_SECOND_STILL_RUNNING")
    });
    let (status, listed) = yinz.run_cli_action_reporting(yinz_cli::args::CliAction::ListPanes {
        tab: false,
        command: false,
        state: false,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(status, 0);
    let panes: serde_json::Value = serde_json::from_str(&listed).expect("pane list JSON");
    let mut ids: Vec<_> = panes
        .as_array()
        .unwrap()
        .iter()
        .filter(|pane| pane["is_plugin"] == false)
        .map(|pane| pane["id"].as_u64().unwrap())
        .collect();
    ids.sort();
    let mut expected = vec![
        u64::from(first.terminal_id().get()),
        u64::from(second.terminal_id().get()),
    ];
    expected.sort();
    assert_eq!(ids, expected);
}

#[test]
fn arrangement_reports_disabled_builtin_services() {
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"$ ");
    assert_eq!(
        yinz.run_cli_action(yinz_cli::args::CliAction::ArrangePanesByProject),
        0
    );
    yinz.wait_until("disabled services are explained", |screen| {
        screen.contains("project-store") && screen.contains("agent-service")
    });
}

#[test]
fn arrangement_loads_chrome_for_a_new_project_tab() {
    let first_repo = write_git_repository("chrome-a", "main");
    let second_repo = write_git_repository("chrome-b", "main");
    let layout = format!(
        r#"layout {{
            default_tab_template {{
                pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
                children
                pane size=2 borderless=true {{ plugin location="yinz:status-bar"; }}
            }}
            new_tab_template {{
                pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
                pane
                pane size=2 borderless=true {{ plugin location="yinz:status-bar"; }}
                floating_panes {{
                    pane x=0 y=1 width=40 height=10 command="default-floating-command"
                    pane x=45 y=1 width=40 height=10 {{ plugin location="yinz:strider"; }}
                }}
            }}
            tab {{
                pane cwd="{}"
                pane cwd="{}" focus=true
            }}
        }}"#,
        first_repo.display(),
        second_repo.display()
    );
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(AGENT_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let first = yinz.expect_pty_spawn();
    let second = yinz.expect_pty_spawn();
    // Normal startup inherits these defaults. Close them before arranging so any
    // floating pane in a destination must have been created by arrangement.
    let floating = yinz.expect_pty_spawn();
    floating.output(b"DEFAULT_FLOATING_COMMAND");
    yinz.wait_until("the initial floating panes are loaded", |screen| {
        screen.contains("DEFAULT_FLOATING_COMMAND")
    });
    let (status, listed) = yinz.run_cli_action_reporting(yinz_cli::args::CliAction::ListPanes {
        tab: false,
        command: false,
        state: false,
        geometry: true,
        all: true,
        json: true,
    });
    assert_eq!(status, 0);
    let panes: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let floating_panes: Vec<_> = panes
        .as_array()
        .unwrap()
        .iter()
        .filter(|pane| pane["is_floating"] == true)
        .collect();
    assert_eq!(floating_panes.len(), 2, "{listed}");
    for pane in floating_panes {
        let kind = if pane["is_plugin"] == true {
            "plugin"
        } else {
            "terminal"
        };
        assert_eq!(
            yinz.run_cli_action(yinz_cli::args::CliAction::ClosePane {
                pane_id: Some(format!("{kind}_{}", pane["id"].as_u64().unwrap())),
            }),
            0
        );
    }
    first.output(b"CHROME_FIRST");
    second.output(b"CHROME_SECOND");
    yinz.wait_until("both projects are identified", |screen| {
        screen.contains("chrome-a") && screen.contains("chrome-b")
    });
    for _ in 0..2 {
        assert_eq!(
            yinz.run_cli_action(yinz_cli::args::CliAction::ArrangePanesByProject),
            0
        );
        yinz.wait_until(
            "the focused project has its own tab and both bars",
            |screen| {
                screen.contains("CHROME_SECOND")
                    && !screen.contains("CHROME_FIRST")
                    && screen
                        .tab_bar_line()
                        .is_some_and(|bar| bar.contains("chrome-b"))
                    && screen
                        .lines()
                        .iter()
                        .rev()
                        .take(2)
                        .any(|line| line.contains("<t> TAB"))
            },
        );
        let (status, listed) =
            yinz.run_cli_action_reporting(yinz_cli::args::CliAction::ListPanes {
                tab: false,
                command: false,
                state: false,
                geometry: true,
                all: true,
                json: true,
            });
        assert_eq!(status, 0);
        let panes: serde_json::Value = serde_json::from_str(&listed).unwrap();
        let panes = panes.as_array().unwrap();
        assert!(
            panes.iter().all(|pane| pane["is_floating"] == false),
            "{listed}"
        );
        let terminals: Vec<_> = panes
            .iter()
            .filter(|pane| pane["is_plugin"] == false)
            .collect();
        let mut ids: Vec<_> = terminals
            .iter()
            .map(|pane| pane["id"].as_u64().unwrap())
            .collect();
        ids.sort();
        let mut expected = vec![
            u64::from(first.terminal_id().get()),
            u64::from(second.terminal_id().get()),
        ];
        expected.sort();
        assert_eq!(ids, expected);
        assert_ne!(terminals[0]["tab_id"], terminals[1]["tab_id"]);
        for terminal in terminals {
            assert_eq!(terminal["pane_y"], 1);
            assert_eq!(terminal["pane_rows"], TERMINAL_SIZE.rows - 3);
            let bars: Vec<_> = panes
                .iter()
                .filter(|pane| {
                    pane["tab_id"] == terminal["tab_id"]
                        && pane["is_plugin"] == true
                        && matches!(
                            pane["plugin_url"].as_str(),
                            Some("yinz:tab-bar" | "yinz:status-bar")
                        )
                })
                .collect();
            assert_eq!(
                bars.len(),
                2,
                "each project tab keeps exactly one pair of bars: {listed}"
            );
        }
    }
}

#[test]
fn registry_updates_palette_and_switching_project_preserves_focus() {
    let folder = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let config = format!(
        r#"{AGENT_PROJECT_CONFIG}
        keybinds {{ shared {{ bind "Ctrl Alt p" {{ ToggleCommandPalette; }}; }}; }}
    "#
    );
    // Keep the focused pane outside these projects so discovery cannot change the selection.
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&config)
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(b"$ ");
    yinz.wait_until("shell ready", |screen| screen.contains("$"));
    yinz.send_stdin(b"\x1b[112;7u");
    yinz.wait_until("empty project context", |screen| {
        screen.contains("Project: None")
    });
    mutate_store(
        &yinz,
        "project_remember",
        serde_json::json!({
            "name": "alpha", "root": folder.path(),
        }),
    );
    yinz.wait_until("registry reaches open palette", |screen| {
        screen.contains("Project: alpha")
    });
    mutate_store(
        &yinz,
        "project_remember",
        serde_json::json!({
            "name": "beta", "root": other.path(),
        }),
    );
    yinz.send_stdin(b"Switch current project");
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("projects without panes appear", |screen| {
        screen.contains("beta") && screen.contains("Current")
    });
    yinz.send_stdin(b"beta");
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("selection clears the previous search", |screen| {
        screen.contains("Project: beta")
            && !screen.contains("Switch current project")
            && !screen.contains("> beta")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("palette closed", |screen| !screen.contains("Do · Project:"));
    yinz.send_stdin(b"still focused");
    shell.wait_for_stdin("input reaches original pane", |bytes| {
        bytes.ends_with(b"still focused")
    });
    yinz.send_stdin(b"\x1b[112;7u");
    yinz.wait_until("selection survives palette lifetime", |screen| {
        screen.contains("Project: beta")
    });
    yinz.quit();
}

#[test]
fn non_git_provider_labels_discovers_groups_and_filters_agents() {
    use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
    use yinz_types::agent::control::{AgentOutcome, AgentRequest};
    let directory = tempfile::tempdir().unwrap();
    let first_root = directory.path().join("first");
    let second_root = directory.path().join("second");
    std::fs::create_dir(&first_root).unwrap();
    std::fs::create_dir(&second_root).unwrap();
    let provider = directory.path().join("provider.scm");
    for root in [&first_root, &second_root] {
        let marker = root.join(".yinz-project");
        std::fs::create_dir(&marker).unwrap();
        std::fs::write(marker.join("identity"), "shared-work").unwrap();
        std::fs::write(marker.join("name"), "MarkerWork").unwrap();
    }
    // Resolve interpreter shims before the harness replaces HOME.
    let python = std::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(
        python.status.success(),
        "resolve Python 3: {}",
        String::from_utf8_lossy(&python.stderr)
    );
    let python = String::from_utf8(python.stdout).unwrap();
    let source = include_str!("../../docs/examples/marker-project.scm")
        .replace("\"python3\"", &serde_json::json!(python.trim()).to_string());
    std::fs::write(&provider, source).unwrap();
    let config = AGENT_PROJECT_CONFIG.replace(
        "  \"project-store\"\n",
        &format!(
            "  \"project-store\"\n  \"file:{}\" {{ shared true; }}\n",
            provider.display()
        ),
    );
    let layout = format!(
        r#"layout {{
      tab {{
        pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
        pane cwd="{}"
      }}
      tab focus=true {{
        pane size=1 borderless=true {{ plugin location="yinz:tab-bar"; }}
        pane cwd="{}"
      }}
    }}"#,
        first_root.display(),
        second_root.display()
    );
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(&config)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let first = yinz.expect_pty_spawn();
    let second = yinz.expect_pty_spawn();
    first.output(b"MARKER_FIRST");
    second.output(b"MARKER_SECOND");
    yinz.wait_until("project provider permission prompt", |screen| {
        screen.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    wait_for_tab(&yinz, "MarkerWork", &[]);
    assert_eq!(yinz.run_cli_action(CliAction::ArrangePanesByProject), 0);
    yinz.wait_until("non-Git roots grouped together", |screen| {
        screen.contains("MARKER_FIRST") && screen.contains("MARKER_SECOND")
    });
    first.run_foreground(&["codex"]);
    second.run_foreground(&["codex"]);
    // Opening Agents observes the saved project, beyond the shell-only label.
    yinz.run_cli_action(CliAction::LaunchOrFocusPlugin {
        url: "yinz:agents".into(),
        configuration: None,
        configuration_json: None,
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        move_to_focused_tab: false,
        skip_plugin_cache: false,
        tab_id: None,
    });
    yinz.wait_until("non-Git agents grouped in the project panel", |screen| {
        screen.contains("MarkerWork") && screen.contains("codex")
    });
    let socket = yinz_ipc::consts::ipc_connect(
        &yinz_runtime::consts::YINZ_SOCK_DIR.join(yinz.session_name()),
    )
    .unwrap();
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
    let mut receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
    receiver
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::List {
            project: Some("MarkerWork".into()),
        }))
        .unwrap();
    let response = loop {
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => break response,
            IpcRecv::Msg(_, _) => {},
            _ => panic!("missing agent list"),
        }
    };
    assert_eq!(response.outcome, AgentOutcome::Ok, "{response:?}");
    assert_eq!(response.agents.len(), 2, "{response:?}");
    yinz.quit();
}

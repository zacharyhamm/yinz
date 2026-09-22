#![cfg(unix)]

//! The tab bar is the one place the project name is actually for.
//!
//! Both ends are unit-tested — hocus works out the label from the agents in a
//! tab, and the server turns labels into names — but they are in different
//! processes with a protobuf between them, and neither test can say the name
//! made it onto the bar. Only a real session with a real grid says that.

use zellij_cli::args::CliAction;
use zellij_integration_tests::test_env::write_git_repository;
use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

/// hocus, the daemon that finds the agents, and the store that turns the
/// checkouts they are in into projects. All three are needed: without the store
/// there is a checkout but no project, and so nothing to name a tab after.
const HOCUS_PROJECT_CONFIG: &str = r#"plugins {
  hocus location="zellij:hocus" {
    shared true
  }
  hocus-agents location="zellij:hocus-agents" {
    shared true
  }
  hocus-agentd location="zellij:hocus-agentd"
  hocus-store location="zellij:hocus-store"
}
load_plugins clear-defaults=true {
  "hocus-agentd"
  "hocus-store"
}
"#;

fn mutate_store(zellij: &TestSession, op: &str, args: serde_json::Value) {
    assert_eq!(
        zellij.run_cli_action(CliAction::Pipe {
            name: Some("store_mutate".into()),
            payload: Some(serde_json::json!({"op": op, "args": args}).to_string()),
            args: None,
            plugin: None,
            plugin_configuration: None,
            force_launch_plugin: false,
            skip_plugin_cache: false,
            floating_plugin: None,
            in_place_plugin: None,
            plugin_cwd: None,
            plugin_title: None,
        }),
        0
    );
}

fn wait_for_tab(zellij: &TestSession, name: &str, absent: &[&str]) {
    zellij.wait_until(&format!("the tab shows {name}"), |screen| {
        screen
            .lines()
            .first()
            .is_some_and(|bar| bar.contains(name) && absent.iter().all(|old| !bar.contains(old)))
    });
}

#[test]
fn a_tab_is_named_after_the_project_its_agent_is_in() {
    let repository = write_git_repository("tab-project", "trunk");
    // The tab bar has to be in the layout: `clear-defaults` above took it away
    // along with everything else, and it is the thing under test. hocus floats below it
    // (`y=1`): an overlay at the top-left row would cover the very line the test reads.
    let layout = format!(
        r#"layout {{
    pane size=1 borderless=true {{
        plugin location="zellij:tab-bar"
    }}
    floating_panes {{
        pane x=0 y=1 width="50%" height="90%" {{
            plugin location="hocus" {{
                overlay true
            }}
        }}
    }}
    pane cwd="{}"
}}
"#,
        repository.display()
    );
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let agent = zellij.expect_pty_spawn();
    agent.run_foreground(&["claude"]);
    agent.output(b"$ ");

    zellij.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(&keys::key('y'));
    zellij.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    // The whole chain: the daemon probes the pane's folder, the store makes a
    // project of it, hocus labels the tab, and the server puts the label on the
    // bar in place of the "Tab #1" the tab was born with.
    let screen = zellij.wait_until("the tab is named after the project", |grid_snapshot| {
        grid_snapshot
            .lines()
            .first()
            .is_some_and(|tab_bar| tab_bar.contains("tab-project"))
    });
    let tab_bar = screen.lines().first().cloned().unwrap_or_default();
    assert!(
        !tab_bar.contains("Tab #1"),
        "the project replaced the default name rather than sitting beside it:\n{screen}"
    );
}

#[test]
fn a_lone_shell_gets_a_project_name_after_tab_creation() {
    let folder = tempfile::tempdir().expect("shell folder");
    let layout = format!(
        r#"layout {{
    pane size=1 borderless=true {{
        plugin location="zellij:tab-bar"
    }}
    pane cwd="{}"
}}
"#,
        folder.path().display()
    );
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let shell = zellij.expect_pty_spawn();
    shell.output(b"$ ");
    zellij.wait_until("the shell tab has its default name", |screen| {
        screen
            .lines()
            .first()
            .is_some_and(|bar| bar.contains("Tab #1"))
    });

    assert_eq!(
        zellij.run_cli_action(CliAction::RenamePane {
            name: "quiet-command".into(),
            pane_id: Some(shell.terminal_id().to_string()),
        }),
        0
    );
    wait_for_tab(&zellij, "quiet-command", &["Tab #1"]);
    mutate_store(
        &zellij,
        "project_remember",
        serde_json::json!({
            "name": "quiet-shell-project", "root": folder.path(),
        }),
    );
    // No focus change or further shell output: the background plugins must publish it.
    zellij.wait_until(
        "the quiet shell tab takes the newly known project",
        |screen| {
            screen
                .lines()
                .first()
                .is_some_and(|bar| bar.contains("quiet-shell-project"))
        },
    );
    assert_eq!(
        zellij.run_cli_action(CliAction::RenameTab {
            name: "my-explicit-name".into(),
            tab_id: None,
        }),
        0
    );
    wait_for_tab(&zellij, "my-explicit-name", &["quiet-shell-project"]);
    mutate_store(
        &zellij,
        "project_forget",
        serde_json::json!({"name": "quiet-shell-project"}),
    );
    mutate_store(
        &zellij,
        "project_remember",
        serde_json::json!({
            "name": "renamed-project", "root": folder.path(),
        }),
    );
    assert_eq!(
        zellij.run_cli_action(CliAction::UndoRenameTab { tab_id: None }),
        0
    );
    wait_for_tab(
        &zellij,
        "renamed-project",
        &["my-explicit-name", "quiet-command"],
    );
    assert_eq!(
        zellij.run_cli_action(CliAction::RenameTab {
            name: "another-name".into(),
            tab_id: None,
        }),
        0
    );
    wait_for_tab(&zellij, "another-name", &["renamed-project"]);
    assert_eq!(
        zellij.run_cli_action(CliAction::RenameTab {
            name: String::new(),
            tab_id: None,
        }),
        0
    );
    wait_for_tab(&zellij, "renamed-project", &["another-name"]);
    mutate_store(
        &zellij,
        "project_forget",
        serde_json::json!({"name": "renamed-project"}),
    );
    wait_for_tab(&zellij, "quiet-command", &["renamed-project"]);
}

#[test]
fn tab_projects_follow_cwd_changes_and_pane_closure_without_hocus() {
    let a = write_git_repository("project-alpha", "main");
    let b = write_git_repository("project-beta", "main");
    let unknown = tempfile::tempdir().unwrap();
    let layout = format!(
        r#"layout {{
        pane size=1 borderless=true {{ plugin location="zellij:tab-bar"; }}
        pane cwd="{}"
        pane cwd="{}"
    }}"#,
        a.display(),
        unknown.path().display()
    );
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let first = zellij.expect_pty_spawn();
    let second = zellij.expect_pty_spawn();
    wait_for_tab(&zellij, "project-alpha", &["project-beta"]);

    // Only the CWD notification, with no prompt output or focus change to trigger a redraw.
    second.output(format!("\x1b]7;file://localhost{}\x07", b.display()).as_bytes());
    wait_for_tab(&zellij, "project-alpha + project-beta", &[]);
    assert_eq!(
        zellij.run_cli_action(CliAction::ClosePane {
            pane_id: Some(first.terminal_id().to_string()),
        }),
        0
    );
    wait_for_tab(&zellij, "project-beta", &["project-alpha"]);
    second.output(format!("\x1b]7;file://localhost{}\x07", unknown.path().display()).as_bytes());
    wait_for_tab(&zellij, "Tab #1", &["project-beta"]);
}

#[test]
fn hidden_panes_keep_naming_the_tab_and_follow_cwd_changes() {
    let a = write_git_repository("hidden-alpha", "main");
    let b = write_git_repository("editor-beta", "main");
    let unknown = tempfile::tempdir().unwrap();
    let layout = format!(
        r#"layout {{
        pane size=1 borderless=true {{ plugin location="zellij:tab-bar"; }}
        pane cwd="{}"
    }}"#,
        a.display()
    );
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "{HOCUS_PROJECT_CONFIG}\nscrollback_editor \"/usr/bin/true\""
        ))
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let shell = zellij.expect_pty_spawn();
    shell.output(b"SCROLLBACK_SOURCE");
    zellij.wait_until("scrollback source is available", |screen| {
        screen.contains("SCROLLBACK_SOURCE")
    });
    wait_for_tab(&zellij, "hidden-alpha", &[]);
    assert_eq!(
        zellij.run_cli_action(CliAction::EditScrollback {
            pane_id: Some(shell.terminal_id().to_string()),
            ansi: false,
        }),
        0
    );
    let editor = zellij.expect_pty_spawn();
    wait_for_tab(
        &zellij,
        &format!("[{}] hidden-alpha", editor.terminal_id()),
        &[],
    );
    editor.output(format!("\x1b]7;file://localhost{}\x07", b.display()).as_bytes());
    wait_for_tab(&zellij, "editor-beta + hidden-alpha", &[]);
    shell.output(format!("\x1b]7;file://localhost{}\x07", unknown.path().display()).as_bytes());
    wait_for_tab(&zellij, "editor-beta", &["hidden-alpha"]);
    editor.exit(Some(0));
    wait_for_tab(&zellij, "Tab #1", &["editor-beta", "hidden-alpha"]);
}

#[test]
fn arrangement_cli_and_tab_binding_keep_both_running_terminals() {
    let repository = write_git_repository("arrangement-project", "main");
    let layout = format!(
        r#"layout {{
        tab {{
            pane size=1 borderless=true {{ plugin location="zellij:tab-bar"; }}
            pane cwd="{}"
        }}
        tab focus=true {{
            pane size=1 borderless=true {{ plugin location="zellij:tab-bar"; }}
            pane cwd="{}"
        }}
    }}"#,
        repository.display(),
        repository.display()
    );
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let first = zellij.expect_pty_spawn();
    let second = zellij.expect_pty_spawn();
    first.output(b"ARRANGE_FIRST");
    second.output(b"ARRANGE_SECOND");
    zellij.wait_until("the store identifies the project", |screen| {
        screen.contains("arrangement-project")
    });
    assert_eq!(
        zellij.run_cli_action(zellij_cli::args::CliAction::ArrangePanesByProject),
        0
    );
    zellij.wait_until("the project terminals share one tab", |screen| {
        screen.contains("ARRANGE_FIRST") && screen.contains("ARRANGE_SECOND")
    });
    zellij.send_stdin(&[keys::ctrl('t'), keys::key('a')].concat());
    first.output(b"_STILL_RUNNING");
    second.output(b"_STILL_RUNNING");
    zellij.wait_until("both processes still render after the binding", |screen| {
        screen.contains("ARRANGE_FIRST_STILL_RUNNING")
            && screen.contains("ARRANGE_SECOND_STILL_RUNNING")
    });
    let (status, listed) =
        zellij.run_cli_action_reporting(zellij_cli::args::CliAction::ListPanes {
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
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .start();
    let terminal = zellij.expect_pty_spawn();
    terminal.output(b"$ ");
    assert_eq!(
        zellij.run_cli_action(zellij_cli::args::CliAction::ArrangePanesByProject),
        0
    );
    zellij.wait_until("disabled services are explained", |screen| {
        screen.contains("hocus-store") && screen.contains("hocus-agentd")
    });
}

#[test]
fn arrangement_loads_chrome_for_a_new_project_tab() {
    let first_repo = write_git_repository("chrome-a", "main");
    let second_repo = write_git_repository("chrome-b", "main");
    let layout = format!(
        r#"layout {{
            default_tab_template {{
                pane size=1 borderless=true {{ plugin location="zellij:tab-bar"; }}
                children
                pane size=2 borderless=true {{ plugin location="zellij:status-bar"; }}
            }}
            new_tab_template {{
                pane size=1 borderless=true {{ plugin location="zellij:tab-bar"; }}
                pane
                pane size=2 borderless=true {{ plugin location="zellij:status-bar"; }}
                floating_panes {{
                    pane x=0 y=1 width=40 height=10 command="default-floating-command"
                    pane x=45 y=1 width=40 height=10 {{ plugin location="zellij:strider"; }}
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
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let first = zellij.expect_pty_spawn();
    let second = zellij.expect_pty_spawn();
    // Normal startup inherits these defaults. Close them before arranging so any
    // floating pane in a destination must have been created by arrangement.
    let floating = zellij.expect_pty_spawn();
    floating.output(b"DEFAULT_FLOATING_COMMAND");
    zellij.wait_until("the initial floating panes are loaded", |screen| {
        screen.contains("DEFAULT_FLOATING_COMMAND")
    });
    let (status, listed) =
        zellij.run_cli_action_reporting(zellij_cli::args::CliAction::ListPanes {
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
            zellij.run_cli_action(zellij_cli::args::CliAction::ClosePane {
                pane_id: Some(format!("{kind}_{}", pane["id"].as_u64().unwrap())),
            }),
            0
        );
    }
    first.output(b"CHROME_FIRST");
    second.output(b"CHROME_SECOND");
    zellij.wait_until("both projects are identified", |screen| {
        screen.contains("chrome-a") && screen.contains("chrome-b")
    });
    for _ in 0..2 {
        assert_eq!(
            zellij.run_cli_action(zellij_cli::args::CliAction::ArrangePanesByProject),
            0
        );
        zellij.wait_until(
            "the focused project has its own tab and both bars",
            |screen| {
                screen.contains("CHROME_SECOND")
                    && !screen.contains("CHROME_FIRST")
                    && screen
                        .lines()
                        .first()
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
            zellij.run_cli_action_reporting(zellij_cli::args::CliAction::ListPanes {
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
                            Some("zellij:tab-bar" | "zellij:status-bar")
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

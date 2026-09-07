#![cfg(unix)]

//! The tab bar is the one place the project name is actually for.
//!
//! Both ends are unit-tested — hocus works out the label from the agents in a
//! tab, and the server turns labels into names — but they are in different
//! processes with a protobuf between them, and neither test can say the name
//! made it onto the bar. Only a real session with a real grid says that.

use zellij_integration_tests::test_env::write_git_repository;
use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TERMINAL_SIZE};

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

    let payload = serde_json::json!({
        "op": "project_remember",
        "args": {"name": "quiet-shell-project", "root": folder.path()},
    });
    assert_eq!(
        zellij.run_cli_action(zellij_cli::args::CliAction::Pipe {
            name: Some("store_mutate".to_owned()),
            payload: Some(payload.to_string()),
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

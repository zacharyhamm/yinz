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
        zellij.run_cli_action(zellij_utils::cli::CliAction::Pipe {
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

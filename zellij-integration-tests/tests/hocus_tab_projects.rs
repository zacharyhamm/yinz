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
    // along with everything else, and it is the thing under test.
    let layout = format!(
        r#"layout {{
    pane size=1 borderless=true {{
        plugin location="zellij:tab-bar"
    }}
    global_panes {{
        pane edge="left" size="50%" focus=true {{
            plugin location="hocus"
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

#![cfg(unix)]

use yinz_cli::args::CliAction;
use yinz_integration_tests::{TestRunner, TERMINAL_SIZE};

fn load_background_plugin_config() -> String {
    let plugin_path = yinz_integration_tests::test_env::fixture_plugin_path();
    format!(
        "plugins {{\n  permission-requester location=\"file:{}\"\n}}\nload_plugins clear-defaults=true {{\n  \"permission-requester\"\n}}\n",
        plugin_path.display()
    )
}

#[test]
fn load_plugins_in_background_on_startup() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&load_background_plugin_config())
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"$ ");

    let grid_snapshot =
        yinz.wait_until("background plugin requests permissions", |grid_snapshot| {
            grid_snapshot.contains("Plugin permissions")
                && grid_snapshot.tab_bar_appears()
                && grid_snapshot.status_bar_appears()
        });
    assert!(grid_snapshot.contains("Deny"));
    let (_, panes) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    let panes: Vec<serde_json::Value> = serde_json::from_str(&panes).unwrap();
    let background = panes
        .iter()
        .find(|pane| pane["is_plugin"] == true && pane["id"] == 1)
        .unwrap();
    assert_eq!(background["is_suppressed"], true);
    assert_eq!(
        panes
            .iter()
            .filter(|pane| pane["is_plugin"] == false)
            .count(),
        1
    );
    yinz.send_stdin(b"\t\r");
    yinz.wait_until(
        "permission prompt dismissed after granting",
        |grid_snapshot| !grid_snapshot.contains("Plugin permissions"),
    );
    yinz.quit();
}

/// `yinz plugin -- file:keys.scm`, the launch the Scheme plugin guide documents: a name, not a
/// path, resolved against the user's own plugin folder.
///
/// It goes out as `CliAction::NewPane { plugin: Some(url), .. }` with the *caller's* cwd, so by
/// the time the server sees it the location is an absolute `$cwd/probe.scm` that is not there.
/// Only the relative name the URL was written with can find the file, which is what this proves
/// - the cwd here is the test process's, and the plugin is somewhere else entirely.
#[test]
fn a_relatively_named_script_plugin_is_launched_from_the_user_plugin_dir() {
    let plugin_dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(
        plugin_dir.path().join("probe.scm"),
        r#"(define (render rows columns) "probe is running")"#,
    )
    .expect("the plugin is written");

    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "user_plugin_dir \"{}\"\n",
            plugin_dir.path().display()
        ))
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"$ ");

    yinz.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec![],
        remote_host: None,

        plugin: Some("file:probe.scm".to_string()),
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    });

    yinz.wait_until("the plugin is found and renders", |grid_snapshot| {
        grid_snapshot.contains("probe is running")
    });
    yinz.quit();
}

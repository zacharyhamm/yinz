#![cfg(unix)]

use insta::assert_snapshot;
use zellij_cli::args::CliAction;
use zellij_integration_tests::{normalized, TestRunner, TERMINAL_SIZE};

fn load_background_plugin_config() -> String {
    let plugin_path = zellij_integration_tests::test_env::fixture_plugin_path();
    format!(
        "plugins {{\n  permission-requester location=\"file:{}\"\n}}\nload_plugins clear-defaults=true {{\n  \"permission-requester\"\n}}\n",
        plugin_path.display()
    )
}

#[test]
fn load_plugins_in_background_on_startup() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&load_background_plugin_config())
        .start();
    let terminal = zellij.expect_pty_spawn();
    terminal.output(b"$ ");

    let grid_snapshot =
        zellij.wait_until("background plugin requests permissions", |grid_snapshot| {
            grid_snapshot.contains("Allow? (y/n)")
                && grid_snapshot.tab_bar_appears()
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("STAGGERED")
        });
    assert_snapshot!(normalized(&grid_snapshot));
    zellij.send_stdin(b"y");
    zellij.wait_until(
        "permission prompt dismissed after granting",
        |grid_snapshot| !grid_snapshot.contains("Allow? (y/n)"),
    );
    zellij.quit();
}

/// `zellij plugin -- file:keys.lua`, the launch the Lua plugin guide documents: a name, not a
/// path, resolved against the user's own plugin folder.
///
/// It goes out as `CliAction::NewPane { plugin: Some(url), .. }` with the *caller's* cwd, so by
/// the time the server sees it the location is an absolute `$cwd/probe.lua` that is not there.
/// Only the relative name the URL was written with can find the file, which is what this proves
/// - the cwd here is the test process's, and the plugin is somewhere else entirely.
#[test]
fn a_relatively_named_lua_plugin_is_launched_from_the_user_plugin_dir() {
    let plugin_dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(
        plugin_dir.path().join("probe.lua"),
        "local M = {}\nfunction M.render() return \"probe is running\" end\nreturn M\n",
    )
    .expect("the plugin is written");

    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "user_plugin_dir \"{}\"\n",
            plugin_dir.path().display()
        ))
        .start();
    let terminal = zellij.expect_pty_spawn();
    terminal.output(b"$ ");

    zellij.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec![],
        remote_host: None,
        firecracker: None,
        firecracker_image: None,
        firecracker_worktree: None,
        plugin: Some("file:probe.lua".to_string()),
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
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

    zellij.wait_until("the plugin is found and renders", |grid_snapshot| {
        grid_snapshot.contains("probe is running")
    });
    zellij.quit();
}

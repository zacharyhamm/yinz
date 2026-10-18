#![cfg(unix)]

use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, split_right_and_wait_for_prompt, start_yinz,
    FakePtyHandle, TestSession,
};

fn replace_pane_in_place_with_held_command(
    yinz: &TestSession,
    pane_id: &str,
    command: &str,
    name: Option<&str>,
) -> FakePtyHandle {
    yinz.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec![command.to_string()],
        remote_host: None,

        plugin: None,
        cwd: None,
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        pane_id: Some(pane_id.to_string()),
        name: name.map(str::to_owned),
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
    yinz.expect_pty_spawn()
}

#[test]
fn lone_pane_title_becomes_the_tab_name() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    terminal.output(b"\x1b]0;my-title\x07");
    let grid_snapshot = yinz.wait_until("tab borrows the pane title", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.contains("my-title")
    });
    assert!(
        !grid_snapshot.contains("Tab #1"),
        "the borrowed pane title replaces the default tab name:\n{}",
        grid_snapshot.text
    );

    yinz.quit();
}

#[test]
fn lone_pane_with_unknown_program_keeps_the_default_tab_name() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    let grid_snapshot = yinz.wait_until("default tab name is shown", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.contains("Tab #1")
    });
    assert!(
        !grid_snapshot.contains("Pane #1"),
        "the default pane name never leaks into the tab name:\n{}",
        grid_snapshot.text
    );

    yinz.quit();
}

#[test]
fn tab_name_reverts_when_a_second_pane_is_opened() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    terminal.output(b"\x1b]0;my-title\x07");
    yinz.wait_until("tab borrows the pane title", |grid_snapshot| {
        grid_snapshot.contains("my-title") && !grid_snapshot.contains("Tab #1")
    });

    split_right_and_wait_for_prompt(&yinz);
    let grid_snapshot = yinz.wait_until(
        "tab name reverts once a second pane exists",
        |grid_snapshot| {
            grid_snapshot
                .tab_bar_line()
                .is_some_and(|tab_bar| tab_bar.contains("Tab #1"))
        },
    );
    assert!(
        grid_snapshot.contains("my-title"),
        "the original pane keeps its title in its frame:\n{}",
        grid_snapshot.text
    );

    yinz.quit();
}

#[test]
fn lone_held_pane_tab_name_shows_exit_status() {
    let mut yinz = start_yinz();
    let shell_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    let command_pane_id = format!("terminal_{}", shell_terminal.terminal_id());
    let command_terminal =
        replace_pane_in_place_with_held_command(&yinz, &command_pane_id, "held-command", None);
    yinz.wait_until("command pane took over the lone tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.tab_bar_appears()
    });

    command_terminal.exit(Some(0));
    yinz.wait_until("tab name carries the exit status", |grid_snapshot| {
        grid_snapshot.contains("EXIT CODE: 0")
    });

    yinz.quit();
}

#[test]
fn launch_captions_yield_to_program_and_osc_but_explicit_names_do_not() {
    for name in [None, Some("explicit launch name")] {
        let mut yinz = start_yinz();
        let shell = claim_first_terminal_and_wait_for_prompt(&yinz);
        let command = replace_pane_in_place_with_held_command(
            &yinz,
            &shell.terminal_id().to_string(),
            "/opt/launch-command",
            name,
        );
        command.run_foreground(&["/usr/bin/vim", "main.rs"]);
        yinz.wait_until(
            "the single pane lends its program or explicit launch name",
            |screen| {
                screen.tab_bar_line().is_some_and(|bar| {
                    bar.contains(name.unwrap_or("vim")) && !bar.contains("main.rs")
                })
            },
        );
        command.output(b"\x1b]2;editing main.rs\x07");
        yinz.wait_until(
            "OSC overrides a generated caption but not an explicit name",
            |screen| {
                screen
                    .tab_bar_line()
                    .is_some_and(|bar| bar.contains(name.unwrap_or("editing main.rs")))
            },
        );
        assert_eq!(
            yinz.run_cli_action(CliAction::RenamePane {
                name: String::new(),
                pane_id: Some(command.terminal_id().to_string()),
            }),
            0
        );
        yinz.wait_until("clearing the name restores the OSC title", |screen| {
            screen
                .tab_bar_line()
                .is_some_and(|bar| bar.contains("editing main.rs"))
        });
        yinz.quit();
    }
}

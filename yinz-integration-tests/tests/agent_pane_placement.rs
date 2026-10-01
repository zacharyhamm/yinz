#![cfg(unix)]
//! Where a spawned agent pane lands, seen from a real session.
//!
//! `Screen::agent_pane_target` is unit-tested against the method, and once through the screen
//! thread; what neither can say is that the tab bar shows the tab the agent was sent to and that
//! the keyboard followed it there. On the harness's 120x24 terminal the viewport is 120x21, and
//! one pane's share of it (1260 cells) already sits under the default 80x24 floor, so the first
//! agent spawned is enough.

use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, start_yinz, TestRunner, PROMPT, TERMINAL_SIZE,
};

fn new_pane_action(command: &[&str]) -> CliAction {
    CliAction::NewPane {
        direction: None,
        command: command.iter().map(|part| part.to_string()).collect(),
        remote_host: None,
        firecracker: None,
        firecracker_image: None,
        firecracker_worktree: None,
        plugin: None,
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
    }
}

#[test]
fn an_agent_spawned_into_a_tab_too_small_to_share_gets_a_tab_of_its_own_and_the_keyboard() {
    let mut yinz = start_yinz();
    let shell = claim_first_terminal_and_wait_for_prompt(&yinz);
    // Something only the shell's tab shows, so its absence later proves that tab is off screen.
    shell.output(b"shellmarker");
    yinz.wait_until("the shell's marker is on screen", |grid_snapshot| {
        grid_snapshot.text.contains("shellmarker")
    });

    yinz.run_cli_action(new_pane_action(&["claude"]));
    let agent = yinz.expect_pty_spawn();
    agent.output(PROMPT);
    // The agent's title on the bar is not enough: until its pending layout lands,
    // the shell's tab bar already lists it while the client still sees the shell.
    // The shell's marker disappearing proves the client moved. A lone pane shows
    // its title in the tab bar; its prompt must also be visible.
    yinz.wait_until(
        "the client is on the agent's tab and sees its prompt",
        |grid_snapshot| {
            grid_snapshot.text.contains("claude")
                && !grid_snapshot.text.contains("shellmarker")
                && grid_snapshot.text.contains("$ ")
        },
    );

    let probe = b"agentprobe";
    yinz.send_stdin(probe);
    agent.wait_for_stdin("the probe reached the agent's pane", |stdin_bytes| {
        stdin_bytes
            .windows(probe.len())
            .any(|window| window == probe)
    });
    yinz.quit();
}

#[test]
fn a_zero_floor_keeps_the_agent_in_the_tab_it_was_asked_for() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("agent_pane_min_columns 0")
        .start();
    let _shell = claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.run_cli_action(new_pane_action(&["claude"]));
    let agent = yinz.expect_pty_spawn();
    agent.output(PROMPT);
    let screen = yinz.wait_until("the agent's prompt beside the shell's", |grid_snapshot| {
        grid_snapshot.text.matches("$").count() >= 2
    });
    assert!(
        !screen.text.contains("Tab #2"),
        "the rule is off, so no tab was made:\n{}",
        screen.text
    );
    yinz.quit();
}

#[test]
fn a_shell_is_not_an_agent_and_splits_the_tab_it_was_asked_for() {
    let mut yinz = start_yinz();
    let _shell = claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.run_cli_action(new_pane_action(&[]));
    let second_shell = yinz.expect_pty_spawn();
    second_shell.output(PROMPT);
    let screen = yinz.wait_until("two prompts in one tab", |grid_snapshot| {
        grid_snapshot.text.matches("$").count() >= 2
    });
    assert!(
        !screen.text.contains("Tab #2"),
        "a shell is placed where it was asked for:\n{}",
        screen.text
    );
    yinz.quit();
}

#![cfg(unix)]

//! Closing the last pane of a tab - by whatever road - never leaves the session broken: the
//! tab goes, and the session with it unless something asked it to stay, in which case the user
//! lands in a fresh tab rather than an empty one.

use std::fs;
use zellij_cli::args::CliAction;
use zellij_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, split_right_and_wait_for_prompt, start_zellij,
    LayoutInfo, TestRunner, TestSession, PROMPT, TERMINAL_SIZE,
};
use zellij_types::pane_size::Size;

fn wait_for_goodbye(zellij: &TestSession, what: &str) {
    zellij.wait_until(what, |grid_snapshot| {
        grid_snapshot.contains("Bye from Zellij!")
    });
}

/// A session built from `layout` alone, with no background plugins to muddy the count of
/// what is in a tab.
fn zellij_with_layout(layout: &str) -> TestSession {
    TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(layout.to_owned()))
        .start()
}

#[test]
fn closing_the_only_pane_from_the_cli_ends_the_session() {
    let mut zellij = start_zellij();
    let terminal = claim_first_terminal_and_wait_for_prompt(&zellij);

    assert_eq!(
        zellij.run_cli_action(CliAction::ClosePane {
            pane_id: Some(format!("terminal_{}", terminal.terminal_id())),
        }),
        0
    );

    wait_for_goodbye(&zellij, "zellij exited after the CLI closed the only pane");
    zellij.wait_for_exit();
}

#[test]
fn closing_the_only_tab_ends_the_session() {
    let mut zellij = start_zellij();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    zellij.send_stdin(&keys::ctrl('t'));
    zellij.send_stdin(&keys::key('x'));

    wait_for_goodbye(&zellij, "zellij exited after the only tab closed");
    zellij.wait_for_exit();
}

#[test]
fn a_floating_pane_as_the_last_pane_ends_the_session_when_it_exits() {
    let mut zellij = zellij_with_layout(
        r#"layout {
    pane
    floating_panes {
        pane x=10 y=2 width=40 height=10
    }
}"#,
    );
    let tiled = zellij.expect_pty_spawn();
    let floating = zellij.expect_pty_spawn();
    tiled.output(PROMPT);
    floating.output(b"floating prompt$ ");
    zellij.wait_until("both panes rendered", |grid_snapshot| {
        grid_snapshot.contains("floating prompt$ ")
    });

    // The tiled pane goes first; the floating one alone keeps the tab, and the session.
    tiled.exit(Some(0));
    floating.output(b"still here\r\n");
    zellij.wait_until("the floating pane survives its tiled neighbour", |s| {
        s.contains("still here")
    });

    floating.exit(Some(0));
    wait_for_goodbye(&zellij, "zellij exited after the last floating pane exited");
    zellij.wait_for_exit();
}

#[test]
fn the_last_member_of_a_stack_ends_the_session_when_it_exits() {
    let mut zellij = zellij_with_layout(
        r#"layout {
    pane stacked=true {
        pane
        pane
    }
}"#,
    );
    let first = zellij.expect_pty_spawn();
    let second = zellij.expect_pty_spawn();
    first.output(PROMPT);
    second.output(PROMPT);
    zellij.wait_until("the stack rendered", |grid_snapshot| {
        grid_snapshot.contains("$ ")
    });

    second.exit(Some(0));
    first.output(b"still here\r\n");
    zellij.wait_until("the remaining member survives", |s| {
        s.contains("still here")
    });

    first.exit(Some(0));
    wait_for_goodbye(&zellij, "zellij exited after the last stack member exited");
    zellij.wait_for_exit();
}

#[test]
fn a_fullscreen_last_pane_ends_the_session_when_it_exits() {
    let mut zellij = start_zellij();
    let first = claim_first_terminal_and_wait_for_prompt(&zellij);
    let second = split_right_and_wait_for_prompt(&zellij);

    zellij.send_stdin(&keys::ctrl('p'));
    zellij.send_stdin(&keys::key('f'));
    second.output(b"fullscreen now\r\n");
    zellij.wait_until("the pane is fullscreen", |grid_snapshot| {
        grid_snapshot.contains("fullscreen now")
    });

    // The hidden pane exits behind the fullscreen one, then the fullscreen one does.
    first.exit(Some(0));
    second.output(b"still here\r\n");
    zellij.wait_until("the fullscreen pane survives", |s| s.contains("still here"));

    second.exit(Some(0));
    wait_for_goodbye(&zellij, "zellij exited after the fullscreen pane exited");
    zellij.wait_for_exit();
}

#[test]
fn a_held_last_pane_keeps_the_session_alive_until_it_is_closed() {
    let mut zellij = start_zellij();
    let shell = claim_first_terminal_and_wait_for_prompt(&zellij);
    assert_eq!(
        zellij.run_cli_action(CliAction::NewPane {
            direction: None,
            command: vec!["held-command".to_owned()],
            remote_host: None,
            firecracker: None,
            firecracker_image: None,
            firecracker_worktree: None,
            plugin: None,
            cwd: None,
            floating: false,
            in_place: true,
            close_replaced_pane: true,
            pane_id: Some(format!("terminal_{}", shell.terminal_id())),
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
        }),
        0
    );
    let command = zellij.expect_pty_spawn();
    command.output(b"working\r\n");
    zellij.wait_until("the command pane took over the lone tab", |s| {
        s.contains("working")
    });

    // Held after its command exits: the tab bar says so, and the session goes on.
    command.exit(Some(0));
    zellij.wait_until("the pane is held after its command exits", |s| {
        s.contains("EXIT CODE: 0")
    });

    zellij.send_stdin(&keys::ctrl('p'));
    zellij.send_stdin(&keys::key('x'));
    wait_for_goodbye(&zellij, "zellij exited after the held pane was closed");
    zellij.wait_for_exit();
}

#[test]
fn a_detached_session_ends_when_its_last_pane_exits() {
    let mut zellij = start_zellij();
    let terminal = claim_first_terminal_and_wait_for_prompt(&zellij);

    zellij.detach_main_client();
    terminal.exit(Some(0));

    // Nobody is attached to see a goodbye; the server itself has to go.
    zellij.wait_for_exit();
}

#[test]
fn every_client_leaves_when_the_last_pane_closes() {
    let mut zellij = start_zellij();
    let terminal = claim_first_terminal_and_wait_for_prompt(&zellij);
    let second_client = zellij.attach_client(TERMINAL_SIZE);
    second_client.wait_until("the second client sees the prompt", |s| s.contains("$ "));

    terminal.exit(Some(0));

    wait_for_goodbye(&zellij, "the first client said goodbye");
    second_client.wait_until("the second client said goodbye", |s| {
        s.contains("Bye from Zellij!")
    });
    zellij.wait_for_exit();
}

/// A background plugin that asks for the quit to be confirmed, the way hocus does mid-reply.
const GUARD_PLUGIN: &str = r#"
local z = require('zellij')
local M = {}
function M.load()
  z.task(function()
    z.await(z.async.permissions { 'change_application_state' })
    z.set_quit_notice { notice = 'the fixture is still working', guard = true }
  end)
end
function M.update() return false end
function M.render() end
return M
"#;

/// Write the fixture to disk with its one permission already granted, so no prompt has to be
/// answered: a background plugin asking would surface as a floating pane, which is exactly the
/// kind of selectable thing these tests must not have around.
fn write_guard_plugin(name: &str) -> String {
    let root = zellij_integration_tests::test_env::init().join(name);
    fs::create_dir_all(&root).unwrap();
    let path = root.join("guard.lua");
    fs::write(&path, GUARD_PLUGIN).unwrap();
    let url = format!("file:{}", path.display());
    let key = zellij_runtime::plugin_location::parse_plugin_location(&url, None)
        .unwrap()
        .to_string();
    let cache_path = zellij_runtime::consts::ZELLIJ_PLUGIN_PERMISSIONS_CACHE.clone();
    fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let mut cache =
        zellij_config::permission::PermissionCache::from_path_or_default(Some(cache_path));
    cache.cache(
        key,
        vec![zellij_types::data::PermissionType::ChangeApplicationState],
    );
    cache.write_to_file().unwrap();
    url
}

fn expect_replacement_shell(zellij: &TestSession, what: &str) {
    let shell = zellij.expect_pty_spawn();
    shell.output(b"fresh shell$ ");
    zellij.wait_until(what, |grid_snapshot| {
        grid_snapshot.contains("fresh shell$ ")
            && grid_snapshot.contains("Quit Zellij?")
            && grid_snapshot.contains("the fixture is still working")
    });
}

#[test]
fn a_guarded_session_replaces_its_last_tab_and_asks_before_quitting() {
    let url = write_guard_plugin("last-pane-guard");
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "load_plugins clear-defaults=true {{ \"{url}\"; }}"
        ))
        .start();
    let terminal = claim_first_terminal_and_wait_for_prompt(&zellij);

    terminal.exit(Some(0));

    // A fresh tab with a shell, and the confirmation over it saying why it is asking.
    expect_replacement_shell(&zellij, "a fresh shell comes up under the confirmation");

    zellij.send_stdin(&keys::ESC);
    zellij.wait_until(
        "cancelling leaves a usable shell in a tab with its chrome",
        |s| {
            !s.contains("Quit Zellij?")
                && s.contains("fresh shell$ ")
                // The replacement is a later tab than the one that closed.
                && s.contains("Tab #2")
                && s.status_bar_appears()
        },
    );

    // Closing that shell with the guard still up asks again.
    zellij.send_stdin(&keys::ctrl('p'));
    zellij.send_stdin(&keys::key('x'));
    expect_replacement_shell(
        &zellij,
        "the second close asks again over another fresh shell",
    );

    zellij.send_stdin(&keys::TAB);
    zellij.send_stdin(&keys::ENTER);
    wait_for_goodbye(&zellij, "confirming the quit ends the session");
    zellij.wait_for_exit();
}

#[test]
fn a_guarded_session_with_nobody_attached_waits_for_a_client() {
    let url = write_guard_plugin("last-pane-guard-detached");
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "load_plugins clear-defaults=true {{ \"{url}\"; }}"
        ))
        .start();
    let terminal = claim_first_terminal_and_wait_for_prompt(&zellij);

    zellij.detach_main_client();
    terminal.exit(Some(0));

    let client = zellij.attach_client(Size { cols: 80, rows: 24 });
    let shell = zellij.expect_pty_spawn();
    shell.output(b"fresh shell$ ");
    client.wait_until("the attaching client gets a fresh shell", |s| {
        s.contains("fresh shell$ ") && s.contains("Quit Zellij?")
    });
    client.send_stdin(&keys::TAB);
    client.send_stdin(&keys::ENTER);
    client.wait_until("confirming the quit ends the session", |s| {
        s.contains("Bye from Zellij!")
    });
    zellij.wait_for_exit();
}

fn replacement_exit_is_held(exit_before_render: bool) {
    let url = write_guard_plugin("replacement-exit-held");
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "load_plugins clear-defaults=true {{ \"{url}\"; }}"
        ))
        .start();
    let terminal = claim_first_terminal_and_wait_for_prompt(&zellij);
    terminal.exit(Some(0));
    let shell = zellij.expect_pty_spawn();
    if !exit_before_render {
        shell.output(b"replacement ready$ ");
        zellij.wait_until("replacement rendered with confirmation", |s| {
            s.contains("replacement ready$") && s.contains("Quit Zellij?")
        });
        zellij.send_stdin(&keys::ESC);
        zellij.wait_until("confirmation cancelled", |s| !s.contains("Quit Zellij?"));
    }
    shell.exit(Some(17));
    zellij.wait_until("replacement exit is held", |s| s.contains("EXIT CODE: 17"));
    if exit_before_render {
        zellij.wait_until("confirmation over the held replacement", |s| {
            s.contains("Quit Zellij?")
        });
        zellij.send_stdin(&keys::ESC);
        zellij.wait_until("confirmation cancelled", |s| !s.contains("Quit Zellij?"));
    }
    zellij.send_stdin(&keys::ctrl('p'));
    zellij.send_stdin(&keys::key('x'));
    expect_replacement_shell(&zellij, "explicit close still replaces and confirms");
    zellij.send_stdin(&keys::TAB);
    zellij.send_stdin(&keys::ENTER);
    wait_for_goodbye(&zellij, "confirmed quit");
    zellij.wait_for_exit();
}

#[test]
fn replacement_shell_natural_exit_is_held() {
    replacement_exit_is_held(false);
}

#[test]
fn replacement_shell_immediate_exit_is_held() {
    replacement_exit_is_held(true);
}

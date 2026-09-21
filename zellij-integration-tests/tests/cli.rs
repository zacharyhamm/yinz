#![cfg(unix)]

use insta::assert_snapshot;
use zellij_cli::args::CliAction;
use zellij_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, normalized,
    split_right_and_wait_for_prompt, start_zellij, tab_bar_shows_no_lone_pane_id, Size, TestRunner,
    PROMPT, TERMINAL_SIZE,
};

#[test]
fn override_layout_from_default_to_compact() {
    let mut zellij = start_zellij();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    zellij.override_layout("compact");

    let grid_snapshot =
        zellij.wait_until("compact bar rendered with session name", |grid_snapshot| {
            grid_snapshot.contains("Zellij (test")
                && grid_snapshot.contains("NORMAL")
                && grid_snapshot.contains("Tab #1")
                && !grid_snapshot.status_bar_appears()
        });
    assert_snapshot!(normalized(&grid_snapshot));
    zellij.quit();
}

#[test]
fn send_command_through_the_cli() {
    let mut zellij = start_zellij();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    zellij.run_suspended_command(&["suspended-command"]);
    zellij.wait_until("suspended command pane waiting to run", |grid_snapshot| {
        grid_snapshot.contains("<Ctrl-c>")
    });

    zellij.send_stdin(&keys::ENTER);
    let command_terminal = zellij.expect_pty_spawn();
    zellij.wait_until("command running", |grid_snapshot| {
        !grid_snapshot.contains("<Ctrl-c>")
    });
    command_terminal.output(b"foo\r\n");
    zellij.wait_until("first run printed foo", |grid_snapshot| {
        grid_snapshot.contains("foo")
    });
    command_terminal.exit(Some(0));
    zellij.wait_until("command pane held again after exit", |grid_snapshot| {
        grid_snapshot.contains("EXIT CODE")
    });

    zellij.send_stdin(&keys::ENTER);
    zellij.wait_until("command re-running", |grid_snapshot| {
        !grid_snapshot.contains("EXIT CODE")
    });
    command_terminal.output(b"foo\r\nfoo\r\n");
    let grid_snapshot = zellij.wait_until("command re-ran", |grid_snapshot| {
        grid_snapshot.text.matches("foo").count() >= 2 && !grid_snapshot.contains("EXIT CODE")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    zellij.quit();
}

#[test]
fn send_blocking_command_through_the_cli() {
    let mut zellij = start_zellij();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    let blocking_command = zellij.run_blocking_floating_command(&["blocking-command"]);
    let floating_terminal = zellij.expect_pty_spawn();
    zellij.wait_until("floating command pane appeared", |grid_snapshot| {
        grid_snapshot.contains("PIN")
    });

    floating_terminal.exit(Some(42));
    assert_eq!(blocking_command.wait_for_exit(), 42);

    let grid_snapshot = zellij.wait_until("floating pane closed on exit", |grid_snapshot| {
        !grid_snapshot.contains("PIN") && grid_snapshot.cursor_is_at(col(2).row(1))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    zellij.quit();
}

/// The server broadcasts `UnblockInputThread` to every client whenever any pane closes, so a
/// plain `--blocking` used to come back with 0 the moment an unrelated pane went away. It has to
/// wait for its own command, and answer with that command's status.
#[test]
fn a_plain_blocking_command_outlives_an_unrelated_pane_closing() {
    let mut zellij = start_zellij();
    claim_first_terminal_and_wait_for_prompt(&zellij);
    let unrelated = split_right_and_wait_for_prompt(&zellij);

    let blocking_command = zellij.run_plain_blocking_command(&["blocking-command"]);
    let command_terminal = zellij.expect_pty_spawn();
    zellij.wait_until("blocking command pane appeared", |grid_snapshot| {
        grid_snapshot.contains("blocking-command")
    });

    zellij.run_cli_action(CliAction::ClosePane {
        pane_id: Some(format!("terminal_{}", unrelated.terminal_id())),
    });
    zellij.wait_until("unrelated pane closed", |grid_snapshot| {
        !grid_snapshot.contains("Pane #2") && grid_snapshot.contains("blocking-command")
    });
    assert!(
        !blocking_command.is_finished(),
        "the blocking client returned on another pane's close"
    );

    command_terminal.exit(Some(7));
    assert_eq!(blocking_command.wait_for_exit(), 7);
    zellij.quit();
}

/// `zellij events` streams the session's state as it changes: a subscriber starts from the
/// state as it stands, a new pane is a pane report and a tab report, and a filter keeps the
/// rest out. (The stream's end with the session is not covered: the harness cannot end a
/// session, since the quit-confirmation modal takes Ctrl+q.)
#[test]
fn events_stream_the_session_state() {
    let mut zellij = start_zellij();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    let everything = zellij.stream_events(&[]);
    let only_panes = zellij.stream_events(&["pane_update"]);
    // A new subscriber is sent the state as it stands.
    everything.wait_for_event("PaneUpdate");
    everything.wait_for_event("TabUpdate");
    only_panes.wait_for_event("PaneUpdate");

    let before = only_panes.streamed_event_types().len();
    split_right_and_wait_for_prompt(&zellij);
    zellij.wait_until("the split reached the subscribers", |_| {
        only_panes.streamed_event_types().len() > before
    });
    assert!(
        only_panes
            .streamed_event_types()
            .iter()
            .all(|t| t == "PaneUpdate"),
        "a filtered stream carries only what was asked for: {:?}",
        only_panes.streamed_event_types()
    );
    assert!(
        everything
            .streamed_event_types()
            .iter()
            .any(|t| t == "TabUpdate"),
        "{:?}",
        everything.streamed_event_types()
    );

    zellij.quit();
}

/// `zellij action agent-report` is how an agent tells the session what it is doing; the record
/// then rides on the pane for every reader, `list-panes --json` included, until it is cleared.
#[test]
fn an_agent_report_is_carried_on_the_pane() {
    let mut zellij = start_zellij();
    let terminal = claim_first_terminal_and_wait_for_prompt(&zellij);
    let pane_id = format!("terminal_{}", terminal.terminal_id());
    let list_panes = || CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    };

    let (exit_code, _) = zellij.run_cli_action_reporting(CliAction::AgentReport {
        pane_id: Some(pane_id.clone()),
        kind: Some("codex".to_owned()),
        state: Some("waiting".to_owned()),
        detail: Some("y/n".to_owned()),
        cwd: None,
        hook_json: false,
        clear: false,
        run_id: None,
    });
    assert_eq!(exit_code, 0);
    let (_, listed) = zellij.run_cli_action_reporting(list_panes());
    let panes: serde_json::Value = serde_json::from_str(&listed)
        .unwrap_or_else(|e| panic!("list-panes --json did not answer with JSON: {e}\n{listed}"));
    let reported = panes
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["id"] == terminal.terminal_id().get() && pane["is_plugin"] == false)
        .unwrap_or_else(|| panic!("pane {pane_id} is not listed: {panes}"));
    assert_eq!(reported["agent"]["kind"], "codex", "{reported}");
    assert_eq!(reported["agent"]["state"], "Waiting", "{reported}");
    assert_eq!(reported["agent"]["source"], "Reported", "{reported}");
    assert_eq!(reported["agent"]["detail"], "y/n", "{reported}");

    // A hook event - what `--hook-json` reads off stdin - is sent on to `zellij events` typed:
    // the state the session derived and the agent's own words, keyed by the pane's number.
    let agent_events = zellij.stream_events(&["agent_event", "pane_update"]);
    // The initial state report confirms the subscription before this one-shot event is sent.
    agent_events.wait_for_event("PaneUpdate");
    let json =
        r#"{"hook_event_name":"Stop","session_id":"cc-1","last_assistant_message":"all green"}"#;
    let hook: zellij_types::agent::HookEvent = serde_json::from_str(json).unwrap();
    let (exit_code, reported) =
        zellij.run_actions_reporting(zellij_types::input::actions::hook_report_actions(
            "claude",
            zellij_types::data::PaneId::Terminal(terminal.terminal_id()),
            &hook,
            json.to_owned(),
        ));
    assert_eq!(exit_code, 0, "{reported}");
    agent_events.wait_for_event("AgentEvent");
    let (_, streamed) = agent_events
        .streamed_events()
        .into_iter()
        .find(|(event_type, _)| event_type == "AgentEvent")
        .unwrap();
    let event: serde_json::Value = serde_json::from_str(&streamed).unwrap();
    let event = &event["AgentEvent"];
    assert_eq!(event["pane_id"], terminal.terminal_id().get(), "{event}");
    assert_eq!(event["kind"], "claude", "{event}");
    assert_eq!(event["hook"], "Stop", "{event}");
    assert_eq!(event["state"], "Idle", "{event}");
    assert_eq!(event["last_message"], "all green", "{event}");
    assert_eq!(event["raw"], json, "{event}");

    zellij.run_cli_action(CliAction::AgentReport {
        pane_id: Some(pane_id.clone()),
        kind: None,
        state: None,
        detail: None,
        cwd: None,
        hook_json: false,
        clear: true,
        run_id: None,
    });
    let (_, listed) = zellij.run_cli_action_reporting(list_panes());
    let panes: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let cleared = panes
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["id"] == terminal.terminal_id().get() && pane["is_plugin"] == false)
        .unwrap();
    assert!(cleared["agent"].is_null(), "{cleared}");
    zellij.quit();
}

/// `zellij action reconfigure` changes the running session: a keybind added from a shell is a
/// keybind the next keypress honours.
#[test]
fn a_reconfigure_from_the_cli_takes_effect_at_once() {
    let mut zellij = start_zellij();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    let exit_code = zellij.run_cli_action(CliAction::Reconfigure {
        kdl: Some(
            "keybinds {\n    normal {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n".to_owned(),
        ),
        file: None,
        write: false,
    });
    assert_eq!(exit_code, 0);

    zellij.send_stdin(&keys::ctrl('y'));
    let new_pane = zellij.expect_pty_spawn();
    new_pane.output(PROMPT);
    zellij.wait_until("the bound key opened a pane", |grid_snapshot| {
        grid_snapshot.contains("Pane #2")
    });
    zellij.quit();
}

/// `--write` persists into `config.d/zellij-managed.kdl` under the session's config dir, one
/// block merged into the next, and never into the user's own config file.
#[test]
fn a_reconfigure_with_write_lands_in_the_managed_drop_in() {
    let config_dir = zellij_integration_tests::test_env::init()
        .join("config")
        .join("managed-writes");
    std::fs::create_dir_all(&config_dir).unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(config_dir.clone())
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    let fragment = "keybinds {\n    normal {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n";
    assert_eq!(
        zellij.run_cli_action(CliAction::Reconfigure {
            kdl: Some(fragment.to_owned()),
            file: None,
            write: true,
        }),
        0
    );
    assert_eq!(
        zellij.run_cli_action(CliAction::SetHook {
            point: "agent_exit".to_owned(),
            url: String::new(),
            write: true,
        }),
        0
    );
    let managed = config_dir.join("config.d").join("zellij-managed.kdl");
    let text = std::fs::read_to_string(&managed)
        .unwrap_or_else(|e| panic!("{} was not written: {e}", managed.display()));
    assert!(text.contains("bind \"Ctrl y\""), "{text}");
    assert!(text.contains("agent_exit \"\""), "{text}");
    assert!(
        std::fs::read_dir(&config_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .all(|e| e.file_name() == "config.d"),
        "nothing but the drop-in folder was written to the config dir"
    );

    // And it is live, not just on disk.
    zellij.send_stdin(&keys::ctrl('y'));
    let new_pane = zellij.expect_pty_spawn();
    new_pane.output(PROMPT);
    zellij.wait_until("the persisted keybind opened a pane", |grid_snapshot| {
        grid_snapshot.contains("Pane #2")
    });
    zellij.quit();
}

#[test]
fn watcher_client_functionality() {
    let mut zellij = start_zellij();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&zellij);
    first_terminal.output(b"\r\nWATCHER_OUTPUT_1\r\n");
    zellij.wait_until("first output line", |grid_snapshot| {
        grid_snapshot.contains("WATCHER_OUTPUT_1")
    });

    let watcher = zellij.attach_watcher(TERMINAL_SIZE);
    watcher.wait_until("watcher connected and sees the output", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.contains("WATCHER_OUTPUT_1")
    });

    zellij.send_stdin(&keys::ctrl('p'));
    zellij.send_stdin(&keys::key('r'));
    let right_terminal = zellij.expect_pty_spawn();
    right_terminal.output(PROMPT);
    watcher.wait_until("watcher sees the split", |grid_snapshot| {
        grid_snapshot.contains("│")
    });

    let ignored_new_tab_from_watcher = [keys::ctrl('t'), keys::key('n')].concat();
    watcher.send_stdin(&ignored_new_tab_from_watcher);

    zellij.detach_main_client();
    first_terminal.output(b"WATCHER_OUTPUT_2\r\nWATCHER_OUTPUT_3\r\nWATCHER_DONE\r\n");
    watcher.wait_until(
        "watcher keeps receiving output while no main client is attached",
        |grid_snapshot| {
            grid_snapshot.contains("WATCHER_DONE")
                && grid_snapshot.contains("│")
                && !grid_snapshot.contains("Tab #2")
        },
    );

    // `tab_bar_appears()` in both predicates below is not decoration. A re-attaching
    // client gets a *new* client id, and plugin panes keep one grid per client, so the
    // tab bar starts blank for it and draws nothing at all until its first `TabUpdate`.
    // Without this clause the status bar alone can satisfy the wait, and the snapshot
    // catches a blank top row — which also makes the `Tab #2` guards below vacuous.
    let main = zellij.attach_client(TERMINAL_SIZE);
    let main_snapshot = main.wait_until(
        "re-attached main converges on the same state",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("WATCHER_DONE")
                && grid_snapshot.contains("│")
                // The tab bar can first replay its old single-pane label.
                && tab_bar_shows_no_lone_pane_id(grid_snapshot)
        },
    );
    assert_snapshot!(normalized(&main_snapshot));

    let watcher_snapshot =
        watcher.wait_until("watcher mirrors the re-attached session", |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("WATCHER_DONE")
                && grid_snapshot.contains("│")
                && !grid_snapshot.contains("Tab #2")
                && tab_bar_shows_no_lone_pane_id(grid_snapshot)
        });
    assert_snapshot!(normalized(&watcher_snapshot));

    main.quit();
    watcher.quit();
    zellij.quit();
}

#[test]
fn watcher_larger_than_the_followed_content_is_padded_to_its_own_size() {
    let mut zellij = start_zellij();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&zellij);
    first_terminal.output(b"\r\nWATCHER_PADDING\r\n");
    zellij.wait_until("followed client rendered", |grid_snapshot| {
        grid_snapshot.contains("WATCHER_PADDING")
    });

    let followed_size = Size {
        cols: TERMINAL_SIZE.cols - 20,
        rows: TERMINAL_SIZE.rows - 4,
    };
    zellij.resize(followed_size);
    zellij.wait_until(
        "followed client re-rendered at its new size",
        move |snapshot| {
            snapshot.contains("WATCHER_PADDING") && snapshot.row_count() == followed_size.rows
        },
    );

    let watcher = zellij.attach_watcher(Size {
        cols: TERMINAL_SIZE.cols + 20,
        rows: TERMINAL_SIZE.rows + 6,
    });
    watcher.wait_until("watcher connected and sees the output", |grid_snapshot| {
        grid_snapshot.contains("Ctrl +") && grid_snapshot.contains("WATCHER_PADDING")
    });

    let clear_below_content = format!("\u{1b}[{};1H\u{1b}[m\u{1b}[J", followed_size.rows + 1);
    let clear_to_end_of_content_line =
        format!("\u{1b}[1;{}H\u{1b}[m\u{1b}[K", followed_size.cols + 1);
    watcher.wait_until_raw_output("watcher output padded to its own size", move |bytes| {
        let output = String::from_utf8_lossy(bytes);
        output.contains(&clear_below_content) && output.contains(&clear_to_end_of_content_line)
    });

    watcher.quit();
    zellij.quit();
}

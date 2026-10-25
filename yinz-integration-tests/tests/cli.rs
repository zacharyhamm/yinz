#![cfg(unix)]

use insta::assert_snapshot;
use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, normalized,
    split_right_and_wait_for_prompt, start_yinz, tab_bar_shows_no_lone_pane_id, Size, TestRunner,
    PROMPT, TERMINAL_SIZE,
};

#[test]
fn a_broadcast_pipe_with_no_plugins_completes() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(yinz_types::data::LayoutInfo::Stringified(
            "layout { pane; }".into(),
        ))
        .start();
    yinz.expect_pty_spawn().output(b"READY WITHOUT PLUGINS\r\n");
    yinz.wait_until("terminal ready without plugins", |grid| {
        grid.contains("READY WITHOUT PLUGINS")
    });
    for stdin in [None, Some(""), Some("input\n")] {
        let (status, output) =
            yinz.run_cli_action_reporting_with_stdin(cli_pipe(None, None), stdin);
        assert_eq!(status, 0);
        assert!(output.is_empty(), "unexpected pipe output: {output}");
    }
    yinz.quit();
}

fn cli_pipe(plugin: Option<&str>, payload: Option<&str>) -> CliAction {
    CliAction::Pipe {
        focus_plugin: false,
        name: Some("test-pipe".into()),
        payload: payload.map(str::to_owned),
        args: None,
        plugin: plugin.map(str::to_owned),
        plugin_configuration: None,
        configuration_json: None,
        force_launch_plugin: false,
        skip_plugin_cache: false,
        floating_plugin: None,
        in_place_plugin: None,
        plugin_cwd: None,
        plugin_title: None,
    }
}

fn pipe_recipient_session() -> (yinz_integration_tests::TestSession, String) {
    use yinz_types::data::PermissionType;

    let root = yinz_integration_tests::test_env::init().join("pipe-recipient");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("recipient.scm");
    std::fs::write(
        &path,
        r#"
(yinz-declare-pane)
(define payload "") (define keys 0)
(define (load configuration)
  (yinz-command (hash "Subscribe" '("Key")))
  (yinz-wait-for-permissions '("ReadCliPipes" "ChangeApplicationState")))
(define (update event)
  (if (and (hash? event) (hash-contains? event "Key"))
      (begin (set! keys (+ keys 1)) #t) #f))
(define (render rows columns)
  (string-append "PIPE PAYLOAD: " payload " KEYS: " (number->string keys)))
(define (pipe message)
  (define id (hash-ref (hash-ref message "source") "Cli"))
  (define body (hash-ref message "payload"))
  (if (and (string? body) (equal? (trim body) "close"))
      (begin
        (yinz-command (hash "BlockCliPipeInput" id))
        (yinz-command (hash "CliPipeOutput" (list id "response\n")))
        (yinz-command (hash "UnblockCliPipeInput" id))
        (yinz-command "CloseSelf"))
      (begin
        (define output (cond [(equal? body "identity") (number->string (hash-ref (yinz-command "GetPluginIds") "plugin_id"))]
                             [(json-null? body) "eof\n"] [else "input\n"]))
        (yinz-command (hash "CliPipeOutput" (list id output)))
        (when (and (not (json-null? body)) (not (equal? body "identity"))) (set! payload body))))
  #t)
"#,
    )
    .unwrap();
    let url = format!("file:{}", path.display());
    let cache_path = yinz_runtime::consts::YINZ_PLUGIN_PERMISSIONS_CACHE.clone();
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let mut cache =
        yinz_config::permission::PermissionCache::from_path_or_default(Some(cache_path));
    cache.cache(
        yinz_runtime::plugin_location::parse_plugin_location(&url, None)
            .unwrap()
            .to_string(),
        vec![
            PermissionType::ReadCliPipes,
            PermissionType::ChangeApplicationState,
        ],
    );
    cache.write_to_file().unwrap();
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "plugins {{ recipient location=\"{url}\"; }}\nload_plugins clear-defaults=true {{}}"
        ))
        .with_layout(yinz_types::data::LayoutInfo::Stringified(
            "layout { pane; }".into(),
        ))
        .start();
    yinz.expect_pty_spawn().output(b"PIPE SESSION READY\r\n");
    yinz.wait_until("pipe session ready", |grid| {
        grid.contains("PIPE SESSION READY")
    });
    (yinz, url)
}

#[test]
fn eof_does_not_replace_a_self_closing_pipe_recipient() {
    let (mut yinz, url) = pipe_recipient_session();
    for address in ["recipient", url.as_str(), "id"] {
        for explicit_payload in [false, true] {
            let (status, id) =
                yinz.run_cli_action_reporting(cli_pipe(Some("recipient"), Some("identity")));
            assert_eq!(status, 0, "{id}");
            let id = id.parse::<u32>().unwrap();
            let destination = if address == "id" {
                format!("plugin_{id}")
            } else {
                address.to_owned()
            };
            let (payload, stdin) = if explicit_payload {
                (Some("close"), "")
            } else {
                (None, "close\n")
            };
            let mut action = cli_pipe(Some(&destination), payload);
            if let CliAction::Pipe { focus_plugin, .. } = &mut action {
                *focus_plugin = true;
            }
            let (status, output) = yinz.run_cli_action_reporting_with_stdin(action, Some(stdin));
            assert_eq!(
                (status, output.as_str()),
                (0, "response\n"),
                "{destination}"
            );
            let (status, panes) = yinz.run_cli_action_reporting(CliAction::ListPanes {
                tab: false,
                command: false,
                state: false,
                geometry: false,
                all: true,
                json: true,
            });
            assert_eq!(status, 0, "{panes}");
            let panes: serde_json::Value = serde_json::from_str(&panes).unwrap();
            assert!(
                panes.as_array().unwrap().iter().all(|pane| {
                    pane["plugin_url"] != "recipient"
                        && pane["plugin_url"].as_str() != Some(url.as_str())
                }),
                "{panes}"
            );
        }
    }
    yinz.quit();
}

#[test]
fn live_pipe_recipients_receive_eof_and_initial_empty_requests_still_load() {
    let (mut yinz, _) = pipe_recipient_session();
    for stdin in [Some(""), None, Some("input\n")] {
        let (status, output) =
            yinz.run_cli_action_reporting_with_stdin(cli_pipe(Some("recipient"), None), stdin);
        let expected = if stdin == Some("input\n") {
            "input\neof\n"
        } else {
            "eof\n"
        };
        assert_eq!((status, output.as_str()), (0, expected));
        // Each iteration starts without a recipient, so both kinds of empty initial
        // request have to load it. A terminal request has no follow-up EOF.
        assert_eq!(
            yinz.run_cli_action_reporting(cli_pipe(Some("recipient"), Some("close"))),
            (0, "response\n".into())
        );
    }
    for destination in [
        "plugin_999999",
        "plugin_invalid",
        "file:unsupported.lua",
        "no-such-alias",
    ] {
        for stdin in [Some(""), Some("input\n"), None] {
            let (status, output) =
                yinz.run_cli_action_reporting_with_stdin(cli_pipe(Some(destination), None), stdin);
            assert_eq!(status, 2, "{destination}, stdin={stdin:?}: {output}");
            assert!(
                !output.is_empty(),
                "missing destination diagnostic for {destination}"
            );
            if destination == "no-such-alias" {
                assert!(output.contains(destination), "{output}");
            }
        }
    }
    yinz.quit();
}

#[test]
fn lazy_plugin_load_failures_reach_cli_with_an_attached_ui() {
    let root = yinz_integration_tests::test_env::init().join("failed-pipe-loads");
    std::fs::create_dir_all(&root).unwrap();
    let invalid = root.join("invalid.scm");
    std::fs::write(&invalid, b"(define broken").unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(yinz_types::data::LayoutInfo::Stringified(
            "layout { pane; }".into(),
        ))
        .start();
    yinz.expect_pty_spawn().output(b"PIPE SESSION READY\r\n");
    yinz.wait_until("UI attached before lazy plugin load", |grid| {
        grid.contains("PIPE SESSION READY")
    });
    for url in [
        format!("file:{}", root.join("missing.scm").display()),
        format!("file:{}", invalid.display()),
        "yinz:not-a-builtin".into(),
    ] {
        let (status, diagnostic) =
            yinz.run_cli_action_reporting(cli_pipe(Some(&url), Some("request")));
        assert_eq!(status, 2, "{url}: {diagnostic}");
        assert!(
            !diagnostic.trim().is_empty(),
            "missing load error for {url}"
        );
        if url == "yinz:not-a-builtin" {
            assert!(
                diagnostic.contains("Failed to resolve plugin: not-a-builtin"),
                "{diagnostic}"
            );
        }
    }
    yinz.quit();
}

#[test]
fn override_layout_from_default_to_compact() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.override_layout("compact");

    let grid_snapshot =
        yinz.wait_until("compact bar rendered with session name", |grid_snapshot| {
            grid_snapshot.contains("Yinz (test")
                && grid_snapshot.contains("NORMAL")
                && grid_snapshot.contains("Tab #1")
                && !grid_snapshot.status_bar_appears()
        });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn send_command_through_the_cli() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.run_suspended_command(&["suspended-command"]);
    yinz.wait_until("suspended command pane waiting to run", |grid_snapshot| {
        grid_snapshot.contains("<Ctrl-c>")
    });

    yinz.send_stdin(&keys::ENTER);
    let command_terminal = yinz.expect_pty_spawn();
    yinz.wait_until("command running", |grid_snapshot| {
        !grid_snapshot.contains("<Ctrl-c>")
    });
    command_terminal.output(b"foo\r\n");
    yinz.wait_until("first run printed foo", |grid_snapshot| {
        grid_snapshot.contains("foo")
    });
    command_terminal.exit(Some(0));
    yinz.wait_until("command pane held again after exit", |grid_snapshot| {
        grid_snapshot.contains("EXIT CODE")
    });

    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("command re-running", |grid_snapshot| {
        !grid_snapshot.contains("EXIT CODE")
    });
    command_terminal.output(b"foo\r\nfoo\r\n");
    let grid_snapshot = yinz.wait_until("command re-ran", |grid_snapshot| {
        grid_snapshot.text.matches("foo").count() >= 2
            && !grid_snapshot.contains("EXIT CODE")
            && !grid_snapshot.contains("<Ctrl-c>")
            && tab_bar_shows_no_lone_pane_id(grid_snapshot)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn send_blocking_command_through_the_cli() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    let blocking_command = yinz.run_blocking_floating_command(&["blocking-command"]);
    let floating_terminal = yinz.expect_pty_spawn();
    yinz.wait_until("floating command pane appeared", |grid_snapshot| {
        grid_snapshot.contains("PIN")
    });

    floating_terminal.exit(Some(42));
    assert_eq!(blocking_command.wait_for_exit(), 42);

    let grid_snapshot = yinz.wait_until("floating pane closed on exit", |grid_snapshot| {
        !grid_snapshot.contains("PIN") && grid_snapshot.cursor_is_at(col(2).row(1))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

/// The server broadcasts `UnblockInputThread` to every client whenever any pane closes, so a
/// plain `--blocking` used to come back with 0 the moment an unrelated pane went away. It has to
/// wait for its own command, and answer with that command's status.
#[test]
fn a_plain_blocking_command_outlives_an_unrelated_pane_closing() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let unrelated = split_right_and_wait_for_prompt(&yinz);

    let blocking_command = yinz.run_plain_blocking_command(&["blocking-command"]);
    let command_terminal = yinz.expect_pty_spawn();
    yinz.wait_until("blocking command pane appeared", |grid_snapshot| {
        grid_snapshot.contains("blocking-command")
    });

    yinz.run_cli_action(CliAction::ClosePane {
        pane_id: Some(format!("terminal_{}", unrelated.terminal_id())),
    });
    yinz.wait_until("unrelated pane closed", |grid_snapshot| {
        !grid_snapshot.contains("Pane #2") && grid_snapshot.contains("blocking-command")
    });
    assert!(
        !blocking_command.is_finished(),
        "the blocking client returned on another pane's close"
    );

    command_terminal.exit(Some(7));
    assert_eq!(blocking_command.wait_for_exit(), 7);
    yinz.quit();
}

/// `yinz events` streams the session's state as it changes: a subscriber starts from the
/// state as it stands, a new pane is a pane report and a tab report, and a filter keeps the
/// rest out.
#[test]
fn events_stream_the_session_state() {
    fn terminal_ids_by_tab(json: &str) -> Vec<(usize, u64)> {
        let event: serde_json::Value = serde_json::from_str(json).unwrap();
        let mut ids = vec![];
        for (tab, panes) in event["PaneUpdate"]["panes"].as_object().unwrap() {
            for pane in panes.as_array().unwrap() {
                if !pane["is_plugin"].as_bool().unwrap() {
                    ids.push((tab.parse().unwrap(), pane["id"].as_u64().unwrap()));
                }
            }
        }
        ids.sort_unstable();
        ids
    }

    let mut yinz = start_yinz();
    let first = claim_first_terminal_and_wait_for_prompt(&yinz);

    let everything = yinz.stream_events(&[]);
    let only_panes = yinz.stream_events(&["pane_update"]);
    // A new subscriber is sent the state as it stands.
    for subscriber in [&everything, &only_panes] {
        subscriber.wait_for_event_matching("PaneUpdate", |json| {
            terminal_ids_by_tab(json) == vec![(0, u64::from(first.terminal_id().get()))]
        });
    }
    let initial_tabs: serde_json::Value =
        serde_json::from_str(&everything.wait_for_event_matching("TabUpdate", |_| true)).unwrap();
    let initial_tabs = initial_tabs["TabUpdate"].as_array().unwrap();
    assert_eq!(initial_tabs.len(), 1);
    assert_eq!(initial_tabs[0]["position"], 0);
    assert_eq!(initial_tabs[0]["selectable_tiled_panes_count"], 1);

    let right = split_right_and_wait_for_prompt(&yinz);
    let expected_panes = vec![
        (0, u64::from(first.terminal_id().get())),
        (0, u64::from(right.terminal_id().get())),
    ];
    for subscriber in [&everything, &only_panes] {
        subscriber.wait_for_event_matching("PaneUpdate", |json| {
            terminal_ids_by_tab(json) == expected_panes
        });
    }
    // The initial TabUpdate is insufficient: the report must describe the split.
    let split_tabs: serde_json::Value =
        serde_json::from_str(&everything.wait_for_event_matching("TabUpdate", |json| {
            let event: serde_json::Value = serde_json::from_str(json).unwrap();
            event["TabUpdate"][0]["selectable_tiled_panes_count"] == 2
        }))
        .unwrap();
    let split_tabs = split_tabs["TabUpdate"].as_array().unwrap();
    assert_eq!(split_tabs.len(), 1);
    assert_eq!(split_tabs[0]["position"], 0);
    assert_eq!(split_tabs[0]["tab_id"], initial_tabs[0]["tab_id"]);
    assert!(
        only_panes
            .streamed_event_types()
            .iter()
            .all(|t| t == "PaneUpdate"),
        "a filtered stream carries only what was asked for: {:?}",
        only_panes.streamed_event_types()
    );
    yinz.quit();
}

/// `yinz action agent-report` is how an agent tells the session what it is doing; the record
/// then rides on the pane for every reader, `list-panes --json` included, until it is cleared.
#[test]
fn an_agent_report_is_carried_on_the_pane() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    let pane_id = format!("terminal_{}", terminal.terminal_id());
    let list_panes = || CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    };

    let (exit_code, _) = yinz.run_cli_action_reporting(CliAction::AgentReport {
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
    let (_, listed) = yinz.run_cli_action_reporting(list_panes());
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

    // A hook event - what `--hook-json` reads off stdin - is sent on to `yinz events` typed:
    // the state the session derived and the agent's own words, keyed by the pane's number.
    let agent_events = yinz.stream_events(&["agent_event", "pane_update"]);
    // The initial state report confirms the subscription before this one-shot event is sent.
    agent_events.wait_for_event("PaneUpdate");
    let json =
        r#"{"hook_event_name":"Stop","session_id":"cc-1","last_assistant_message":"all green"}"#;
    let hook: yinz_types::agent::HookEvent = serde_json::from_str(json).unwrap();
    let (exit_code, reported) =
        yinz.run_actions_reporting(yinz_types::input::actions::hook_report_actions(
            "claude",
            yinz_types::data::PaneId::Terminal(terminal.terminal_id()),
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
    assert_eq!(event["state"], "Busy", "{event}");
    assert_eq!(event["last_message"], "all green", "{event}");
    assert_eq!(event["raw"], json, "{event}");

    yinz.run_cli_action(CliAction::AgentReport {
        pane_id: Some(pane_id.clone()),
        kind: None,
        state: None,
        detail: None,
        cwd: None,
        hook_json: false,
        clear: true,
        run_id: None,
    });
    let (_, listed) = yinz.run_cli_action_reporting(list_panes());
    let panes: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let cleared = panes
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["id"] == terminal.terminal_id().get() && pane["is_plugin"] == false)
        .unwrap();
    assert!(cleared["agent"].is_null(), "{cleared}");
    yinz.quit();
}

/// A CLI connection has no mode of its own. With a client attached that has not pressed a key
/// yet, `yinz action switch-mode` switches that client instead of reporting success for nothing.
#[test]
fn switch_mode_from_the_cli_acts_for_a_client_that_has_not_pressed_a_key() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    let (exit_code, reported) = yinz.run_cli_action_reporting(CliAction::SwitchMode {
        input_mode: yinz_types::data::InputMode::Pane,
    });
    assert_eq!(exit_code, 0, "{reported}");
    yinz.wait_until("pane mode active", |grid_snapshot| {
        grid_snapshot.contains("PANE") && !grid_snapshot.contains("LOCK")
    });
    yinz.quit();
}

/// `yinz action reconfigure` changes the running session: a keybind added from a shell is a
/// keybind the next keypress honours.
#[test]
fn a_reconfigure_from_the_cli_takes_effect_at_once() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    let exit_code = yinz.run_cli_action(CliAction::Reconfigure {
        kdl: Some(
            "keybinds {\n    normal {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n".to_owned(),
        ),
        file: None,
        write: false,
    });
    assert_eq!(exit_code, 0);

    yinz.send_stdin(&keys::ctrl('y'));
    let new_pane = yinz.expect_pty_spawn();
    new_pane.output(PROMPT);
    yinz.wait_until("the bound key opened a pane", |grid_snapshot| {
        grid_snapshot.contains("Pane #2")
    });
    yinz.quit();
}

/// `--write` persists into `config.d/yinz-managed.kdl` under the session's config dir, one
/// block merged into the next, and never into the user's own config file.
#[test]
fn a_reconfigure_with_write_lands_in_the_managed_drop_in() {
    let config_dir = yinz_integration_tests::test_env::init()
        .join("config")
        .join("managed-writes");
    std::fs::create_dir_all(&config_dir).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(config_dir.clone())
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    let fragment = "keybinds {\n    normal {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n";
    assert_eq!(
        yinz.run_cli_action(CliAction::Reconfigure {
            kdl: Some(fragment.to_owned()),
            file: None,
            write: true,
        }),
        0
    );
    assert_eq!(
        yinz.run_cli_action(CliAction::SetHook {
            point: "agent_exit".to_owned(),
            url: String::new(),
            write: true,
        }),
        0
    );
    let managed = config_dir.join("config.d").join("yinz-managed.kdl");
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
    yinz.send_stdin(&keys::ctrl('y'));
    let new_pane = yinz.expect_pty_spawn();
    new_pane.output(PROMPT);
    yinz.wait_until("the persisted keybind opened a pane", |grid_snapshot| {
        grid_snapshot.contains("Pane #2")
    });
    yinz.quit();
}

#[test]
fn watcher_client_functionality() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    first_terminal.output(b"\r\nWATCHER_OUTPUT_1\r\n");
    yinz.wait_until("first output line", |grid_snapshot| {
        grid_snapshot.contains("WATCHER_OUTPUT_1")
    });

    let watcher = yinz.attach_watcher(TERMINAL_SIZE);
    watcher.wait_until("watcher connected and sees the output", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.contains("WATCHER_OUTPUT_1")
    });

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('r'));
    let right_terminal = yinz.expect_pty_spawn();
    right_terminal.output(PROMPT);
    watcher.wait_until("watcher sees the split", |grid_snapshot| {
        grid_snapshot.contains("│")
    });

    let ignored_new_tab_from_watcher = [keys::ctrl('t'), keys::key('n')].concat();
    watcher.send_stdin(&ignored_new_tab_from_watcher);

    yinz.detach_main_client();
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
    let main = yinz.attach_client(TERMINAL_SIZE);
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
    yinz.quit();
}

#[test]
fn watcher_larger_than_the_followed_content_has_blank_margins_without_padding() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    first_terminal.output(b"\r\nWATCHER_MARGINS\r\n");
    yinz.wait_until("followed client rendered", |grid_snapshot| {
        grid_snapshot.contains("WATCHER_MARGINS")
    });

    let followed_size = Size {
        cols: TERMINAL_SIZE.cols - 20,
        rows: TERMINAL_SIZE.rows - 4,
    };
    yinz.resize(followed_size);
    yinz.wait_until(
        "followed client re-rendered at its new size",
        move |snapshot| {
            snapshot.contains("WATCHER_MARGINS") && snapshot.row_count() == followed_size.rows
        },
    );

    let watcher = yinz.attach_watcher(Size {
        cols: TERMINAL_SIZE.cols + 20,
        rows: TERMINAL_SIZE.rows + 6,
    });
    let snapshot = watcher.wait_until("watcher connected and sees the output", |grid_snapshot| {
        grid_snapshot.contains("Ctrl +") && grid_snapshot.contains("WATCHER_MARGINS")
    });
    for (row, line) in snapshot.text.lines().enumerate() {
        let content_cols = if row < followed_size.rows {
            followed_size.cols
        } else {
            0
        };
        assert!(
            line.chars().skip(content_cols).all(|c| c == ' '),
            "{line:?}"
        );
    }

    let clear_below_content = format!("\u{1b}[{};1H\u{1b}[m\u{1b}[J", followed_size.rows + 1);
    let clear_to_end_of_content_line =
        format!("\u{1b}[1;{}H\u{1b}[m\u{1b}[K", followed_size.cols + 1);
    let bytes = watcher.raw_bytes();
    let output = String::from_utf8_lossy(&bytes);
    assert!(output.contains("\x1b[2J"));
    assert!(!output.contains(&clear_below_content));
    assert!(!output.contains(&clear_to_end_of_content_line));

    watcher.quit();
    yinz.quit();
}

#[test]
fn normalized_lifecycle_stream_is_opt_in_and_ready_before_live_updates() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    let pane = format!("terminal_{}", terminal.terminal_id());
    let lifecycle = yinz.stream_events(&["agent_lifecycle"]);
    let legacy = yinz.stream_events(&[]);
    legacy.wait_for_event("PaneUpdate");
    let ready: serde_json::Value = serde_json::from_str(
        &lifecycle.wait_for_event_matching("agent_lifecycle", |json| {
            serde_json::from_str::<serde_json::Value>(json).unwrap()["event_type"] == "ready"
        }),
    )
    .unwrap();
    assert!(ready["cursor"].is_string());
    let (code, _) = yinz.run_cli_action_reporting(CliAction::AgentReport {
        pane_id: Some(pane.clone()),
        kind: Some("codex".into()),
        state: Some("waiting".into()),
        detail: Some("private prompt".into()),
        cwd: None,
        hook_json: false,
        clear: false,
        run_id: None,
    });
    assert_eq!(code, 0);
    let state: serde_json::Value = serde_json::from_str(&lifecycle.wait_for_event_matching(
        "agent_lifecycle",
        |json| {
            let value: serde_json::Value = serde_json::from_str(json).unwrap();
            value["pane_id"] == pane
                && value["event_type"] == "state_changed"
                && value["state"] == "waiting"
        },
    ))
    .unwrap();
    assert!(state["sequence"].as_u64() > ready["sequence"].as_u64());
    assert!(!state.to_string().contains("private prompt"));
    assert!(!legacy
        .streamed_event_types()
        .iter()
        .any(|event| event == "agent_lifecycle"));
    yinz.quit();
}

fn pipe_panes(yinz: &yinz_integration_tests::TestSession) -> Vec<yinz_types::data::PaneListEntry> {
    let (status, json) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(status, 0, "{json}");
    // Provider services are suppressed panes even when load_plugins is cleared.
    serde_json::from_str::<Vec<yinz_types::data::PaneListEntry>>(&json)
        .unwrap()
        .into_iter()
        .filter(|pane| !pane.pane_info.is_suppressed)
        .collect()
}

#[test]
fn pipe_focus_only_reveals_and_selects_new_panes() {
    for focus in [false, true] {
        let (mut yinz, url) = pipe_recipient_session();
        let tab_id = Some(pipe_panes(&yinz)[0].tab_id);
        let send = |destination: &str, payload: &str, focus: bool, force: bool| {
            let mut action = cli_pipe(Some(destination), Some(payload));
            if let CliAction::Pipe {
                focus_plugin,
                force_launch_plugin,
                ..
            } = &mut action
            {
                *focus_plugin = focus;
                *force_launch_plugin = force;
            }
            assert_eq!(yinz.run_cli_action_reporting(action).0, 0);
        };
        let visible = || {
            let (status, output) =
                yinz.run_cli_action_reporting(CliAction::AreFloatingPanesVisible { tab_id });
            assert!(
                matches!(output.trim(), "true" | "false"),
                "{status}: {output}"
            );
            status == 0
        };
        assert!(!visible());
        send("recipient", "cold", focus, false);
        yinz.wait_until("cold pipe pane exists with requested focus", |_| {
            let panes = pipe_panes(&yinz);
            panes.len() == 2
                && panes
                    .iter()
                    .any(|p| p.pane_info.is_plugin() && p.pane_info.is_focused == focus)
        });
        assert_eq!(visible(), focus);
        let panes = pipe_panes(&yinz);
        let pane = &panes
            .iter()
            .find(|p| p.pane_info.is_plugin())
            .unwrap()
            .pane_info;
        let id = pane.id.to_string();
        assert!(pane.is_floating && !pane.is_suppressed);
        if !focus {
            assert!(panes
                .iter()
                .any(|p| !p.pane_info.is_plugin() && p.pane_info.is_focused));
            assert!(!yinz.snapshot().contains("PIPE PAYLOAD: cold"));
            assert_eq!(
                yinz.run_cli_action(CliAction::ShowFloatingPanes { tab_id }),
                0
            );
        }
        yinz.wait_until("cold pipe payload visible", |g| {
            g.contains("PIPE PAYLOAD: cold KEYS: 0")
        });
        // The pipe client has exited; the UI still owns the recipient and can type into it.
        yinz.send_stdin(b"x");
        yinz.wait_until("pipe recipient accepts UI input", |g| g.contains("KEYS: 1"));
        for (destination, payload) in [
            (url.as_str(), "warm-url"),
            ("recipient", "warm-alias"),
            (id.as_str(), "warm-id"),
        ] {
            assert_eq!(
                yinz.run_cli_action(CliAction::HideFloatingPanes { tab_id }),
                0
            );
            send(destination, payload, true, false);
            assert!(!visible());
            let panes = pipe_panes(&yinz);
            assert_eq!(panes.len(), 2);
            assert!(panes
                .iter()
                .any(|p| !p.pane_info.is_plugin() && p.pane_info.is_focused));
            assert_eq!(
                yinz.run_cli_action(CliAction::ShowFloatingPanes { tab_id }),
                0
            );
            yinz.wait_until("warm pipe updated the same pane", |g| {
                g.contains(&format!("PIPE PAYLOAD: {payload} KEYS: 1"))
            });
        }
        // A visible floating layer is preserved and its focused pane is not displaced by default.
        send("recipient", "unfocused-new", false, true);
        yinz.wait_until("new pipe pane preserves existing floating focus", |_| {
            let panes = pipe_panes(&yinz);
            panes.len() == 3
                && panes
                    .iter()
                    .any(|p| p.pane_info.id.to_string() == id && p.pane_info.is_focused)
        });
        assert!(visible());
        send("recipient", "forced-focus", true, true);
        yinz.wait_until("forced new pipe pane is focused", |g| {
            g.contains("PIPE PAYLOAD: forced-focus KEYS: 0")
        });
        let panes = pipe_panes(&yinz);
        assert_eq!(panes.len(), 4);
        assert!(panes.iter().any(|p| p.pane_info.is_plugin()
            && p.pane_info.id.to_string() != id
            && p.pane_info.is_focused));
        yinz.quit();
    }
}

/// `list-panes --json` gathers every terminal pane's command and folder from the pty thread in
/// one batch; each pane must still get its own answer, not a neighbour's or none.
#[test]
fn list_panes_json_reports_each_panes_command_and_cwd() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let mut expected = Vec::new();
    for index in 1..=3 {
        let command = format!("listed-{index}");
        let cwd = format!("/work/listed-{index}");
        yinz.run_cli_action(CliAction::NewPane {
            direction: None,
            command: vec![command.clone()],
            remote_host: None,
            plugin: None,
            cwd: Some(cwd.clone().into()),
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
            block_until_exit: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        });
        let pane = yinz.expect_pty_spawn();
        expected.push((pane.terminal_id().get(), command, cwd));
    }

    let (status, listed) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: false,
        geometry: false,
        all: false,
        json: true,
    });
    assert_eq!(status, 0, "{listed}");
    let panes: serde_json::Value = serde_json::from_str(&listed)
        .unwrap_or_else(|e| panic!("list-panes --json did not answer with JSON: {e}\n{listed}"));
    for (terminal_id, command, cwd) in expected {
        let pane = panes
            .as_array()
            .unwrap()
            .iter()
            .find(|pane| pane["id"] == terminal_id && pane["is_plugin"] == false)
            .unwrap_or_else(|| panic!("terminal {terminal_id} is not listed: {panes}"));
        assert_eq!(pane["pane_cwd"], cwd, "{pane}");
        assert!(
            pane["pane_command"]
                .as_str()
                .is_some_and(|listed| listed.contains(&command)),
            "{pane}"
        );
    }
    yinz.quit();
}

use super::*;

fn open_editor(
    yinz: &TestSession,
    finder: &FakePtyHandle,
    selection_file: &PathBuf,
) -> FakePtyHandle {
    let chosen = "/tmp/finder-detach.txt";
    choose(finder, selection_file, chosen);
    let editor = yinz.expect_pty_spawn();
    match editor.terminal_action() {
        Some(TerminalAction::OpenFile(file)) => assert_eq!(file.path, Path::new(chosen)),
        other => panic!("expected the original finder's editor, got {other:?}"),
    }
    editor.output(b"ORIGINAL EDITOR");
    editor
}

fn detach_with_owned_pane(editor_open: bool, another_client: bool) {
    let (mut yinz, shell) = start();
    let (finder, selection_file) = open_finder(&yinz);
    finder.output(b"ORIGINAL FINDER");
    let editor = editor_open.then(|| open_editor(&yinz, &finder, &selection_file));
    let marker = if editor_open {
        "ORIGINAL EDITOR"
    } else {
        "ORIGINAL FINDER"
    };
    yinz.wait_until("the owned pane is ready before detach", |grid| {
        grid.contains(marker)
    });
    let other = another_client.then(|| {
        let client = yinz.attach_client(TERMINAL_SIZE);
        client.wait_until("the second client sees the existing pane", |grid| {
            grid.contains(marker)
        });
        client
    });
    yinz.detach_main_client();
    // A reattaching sole client gets a new ID and the departed client's focus; an already
    // attached client takes over with its own ID.
    let client = other.unwrap_or_else(|| yinz.attach_client(TERMINAL_SIZE));
    client.wait_until("the owned pane survives its owner's detach", |grid| {
        grid.contains(marker)
    });
    let editor = editor.unwrap_or_else(|| open_editor(&yinz, &finder, &selection_file));
    editor.output(b"\r\nEDITOR AFTER DETACH");
    client.wait_until("the same editor remains usable after detach", |grid| {
        grid.contains("EDITOR AFTER DETACH") && !grid.contains(FINDER_TITLE)
    });
    client.send_stdin(b"x");
    editor.wait_for_stdin("input still reaches the original editor", |bytes| {
        bytes.contains(&b'x')
    });
    editor.exit(Some(0));
    shell.output(b"\r\nSHELL AFTER EDITOR");
    client.wait_until(
        "the driver closes its editor and restores the shell",
        |grid| {
            grid.contains("SHELL AFTER EDITOR")
                && grid.status_bar_appears()
                && !grid.contains("EDITOR AFTER DETACH")
                && !grid.contains(FINDER_TITLE)
        },
    );
    assert_cleaned_up(
        &selection_file,
        "the retained driver cleans up its selection",
    );
    client.quit();
    yinz.quit();
}

#[test]
fn finder_survives_last_client_detach_and_reconnect() {
    detach_with_owned_pane(false, false);
}

#[test]
fn editor_survives_last_client_detach_and_reconnect() {
    detach_with_owned_pane(true, false);
}

#[test]
fn finder_survives_owner_detach_with_another_client() {
    detach_with_owned_pane(false, true);
}

#[test]
fn editor_survives_owner_detach_with_another_client() {
    detach_with_owned_pane(true, true);
}

/// Every pane a `PaneUpdate` stream reported, as (is_plugin, id, plugin_url).
fn reported_panes(events: &[(String, String)]) -> Vec<(bool, u64, String)> {
    let mut reported = vec![];
    for (event_type, json) in events {
        if event_type != "PaneUpdate" {
            continue;
        }
        let event: serde_json::Value = serde_json::from_str(json).unwrap();
        for panes in event["PaneUpdate"]["panes"].as_object().unwrap().values() {
            for pane in panes.as_array().unwrap() {
                reported.push((
                    pane["is_plugin"].as_bool().unwrap(),
                    pane["id"].as_u64().unwrap(),
                    pane["plugin_url"].as_str().unwrap_or_default().to_owned(),
                ));
            }
        }
    }
    reported
}

/// The finder's driver declares no pane: from the first `Alt e` on, no pane report ever lists
/// it, and pressing `Alt e` again finds the running driver rather than starting a second one.
#[test]
fn the_driver_never_has_a_pane_and_a_second_press_keeps_one_instance() {
    let (mut yinz, _shell) = start();
    let events = yinz.stream_events(&["pane_update"]);
    events.wait_for_event("PaneUpdate");
    let (finder, _selection_file) = open_finder(&yinz);
    let finder_id = u64::from(finder.terminal_id().get());
    // The stream carried the finder's own pane, so it was live while the driver ran.
    events.wait_for_event_matching("PaneUpdate", |json| {
        reported_panes(&[("PaneUpdate".into(), json.into())])
            .iter()
            .any(|(is_plugin, id, _)| !is_plugin && *id == finder_id)
    });

    yinz.send_stdin(&keys::alt('e'));
    // The client sends nothing more until the second launch has been answered, so a keystroke
    // that reaches the finder's pane proves that launch was handled.
    yinz.send_stdin(b"q");
    finder.wait_for_stdin(
        "typing after the second press reaches the finder",
        |bytes| bytes.contains(&b'q'),
    );
    let drivers: Vec<_> = yinz_client::store_client::plugin_request(
        &yinz.session_name().into(),
        yinz_types::cli::PluginRequest {
            operation: yinz_types::cli::PluginOperation::List,
            target: None,
            configuration: None,
            fallback: None,
            candidates: vec![],
            caller: None,
        },
    )
    .unwrap()
    .plugins
    .into_iter()
    .filter(|status| status.location.ends_with("file-finder"))
    .collect();
    assert_eq!(drivers.len(), 1, "{drivers:#?}");
    assert_eq!(drivers[0].pane, Some(false), "{drivers:#?}");

    finder.exit(Some(130));
    yinz.wait_until("the overlay is dismissed", |grid_snapshot| {
        !grid_snapshot.contains(FINDER_TITLE) && grid_snapshot.status_bar_appears()
    });
    // Wait for the report that drops the finder's pane, so the stream has caught up with
    // everything the driver did.
    let lists_finder = |json: &str| {
        reported_panes(&[("PaneUpdate".into(), json.into())])
            .iter()
            .any(|(is_plugin, id, _)| !is_plugin && *id == finder_id)
    };
    let deadline = std::time::Instant::now() + yinz_integration_tests::default_timeout();
    loop {
        let streamed = events.streamed_events();
        let last_with_finder = streamed
            .iter()
            .rposition(|(_, json)| lists_finder(json))
            .expect("the finder's pane was reported");
        if streamed[last_with_finder + 1..]
            .iter()
            .any(|(event_type, _)| event_type == "PaneUpdate")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no pane report followed the finder's exit: {streamed:?}"
        );
        std::thread::yield_now();
    }
    let reported = reported_panes(&events.streamed_events());
    let driver_panes: Vec<_> = reported
        .iter()
        .filter(|(is_plugin, _, url)| *is_plugin && url.contains("file-finder"))
        .collect();
    assert!(
        driver_panes.is_empty(),
        "a pane report listed the finder's driver: {driver_panes:?}"
    );
    yinz.quit();
}

/// Whether `list-panes` reports `pane` as focused. A tiled pane keeps its focus under a focused
/// floating one, so it is one answer per pane rather than one pane per answer.
fn is_focused(yinz: &TestSession, pane: &FakePtyHandle) -> bool {
    let (status, json) = yinz.run_cli_action_reporting(yinz_cli::args::CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(status, 0, "{json}");
    let id = yinz_types::pane_id::PaneId::Terminal(pane.terminal_id());
    serde_json::from_str::<Vec<yinz_types::data::PaneListEntry>>(&json)
        .unwrap()
        .into_iter()
        .any(|entry| entry.pane_info.id == id && entry.pane_info.is_focused)
}

/// Focuses `shell`, which takes the keyboard from `away_from`.
fn focus_shell(yinz: &TestSession, shell: &FakePtyHandle, away_from: &FakePtyHandle) {
    assert_eq!(
        yinz.run_cli_action(yinz_cli::args::CliAction::FocusPaneId {
            pane_id: yinz_types::pane_id::PaneId::Terminal(shell.terminal_id()).to_string(),
        }),
        0
    );
    yinz.wait_until("focus moves to the shell", |_| {
        is_focused(yinz, shell) && !is_focused(yinz, away_from)
    });
}

/// The driver has no pane, so a second `Alt e` finds nothing to focus. Once the user has moved
/// away, it still brings back what the driver shows: the finder, and later the editor that
/// replaced it, rather than being swallowed.
#[test]
fn a_second_press_after_moving_away_refocuses_the_finder_and_then_its_editor() {
    let (mut yinz, shell) = start();
    let (finder, selection_file) = open_finder(&yinz);
    yinz.wait_until("the finder is focused", |_| is_focused(&yinz, &finder));

    focus_shell(&yinz, &shell, &finder);
    yinz.send_stdin(&keys::alt('e'));
    yinz.wait_until("the second press refocuses the finder", |_| {
        is_focused(&yinz, &finder)
    });
    yinz.send_stdin(b"q");
    finder.wait_for_stdin("typing reaches the refocused finder", |bytes| {
        bytes.contains(&b'q')
    });

    let editor = open_editor(&yinz, &finder, &selection_file);
    yinz.wait_until("the editor is focused", |_| is_focused(&yinz, &editor));
    focus_shell(&yinz, &shell, &editor);
    yinz.send_stdin(&keys::alt('e'));
    yinz.wait_until("the second press refocuses the editor", |_| {
        is_focused(&yinz, &editor)
    });
    yinz.send_stdin(b"w");
    editor.wait_for_stdin("typing reaches the refocused editor", |bytes| {
        bytes.contains(&b'w')
    });

    editor.exit(Some(0));
    yinz.wait_until("the editor closes", |grid| {
        !grid.contains("ORIGINAL EDITOR") && grid.status_bar_appears()
    });
    yinz.quit();
}

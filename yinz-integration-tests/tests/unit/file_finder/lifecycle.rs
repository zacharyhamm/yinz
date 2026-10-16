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

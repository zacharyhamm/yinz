use crate::{keys, TestRunner, TERMINAL_SIZE};

#[test]
fn group_shortcuts_accept_kitty_and_legacy_input_without_a_prefix() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE).start();
    let first = yinz.expect_pty_spawn();
    first.output(b"FIRST_GROUP");
    yinz.wait_until("first group ready", |grid| grid.contains("FIRST_GROUP"));
    yinz.send_stdin(b"\x1b[104;4u\x1b[108;4uone-group");
    first.wait_for_stdin(
        "single-group shortcuts leave input in the same pane",
        |bytes| bytes.ends_with(b"one-group"),
    );
    assert_eq!(first.stdin_bytes(), b"one-group");

    yinz.send_stdin(b"\x14g");
    let second = yinz.expect_pty_spawn();
    second.output(b"SECOND_GROUP");
    yinz.wait_until("second group ready", |grid| grid.contains("SECOND_GROUP"));

    for (previous, next) in [
        (b"\x1bH".as_slice(), b"\x1bL".as_slice()),
        (b"\x1b[104;4u", b"\x1b[108;4u"),
        (b"\x1b[104:72;4:1u", b"\x1b[108:76;4:1u"),
        (b"\x1b[72;3u", b"\x1b[76;3u"),
    ] {
        yinz.send_stdin(previous);
        yinz.wait_until("previous group focused without Ctrl+T", |grid| {
            grid.contains("FIRST_GROUP") && !grid.contains("SECOND_GROUP")
        });
        yinz.send_stdin(previous);
        yinz.wait_until("previous group wraps", |grid| grid.contains("SECOND_GROUP"));
        yinz.send_stdin(next);
        yinz.wait_until("next group wraps", |grid| grid.contains("FIRST_GROUP"));
        yinz.send_stdin(next);
        yinz.wait_until("next group focused", |grid| grid.contains("SECOND_GROUP"));
    }

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(b"\x1b[104;4u");
    yinz.wait_until("group shortcut also works in Tab mode", |grid| {
        grid.contains("FIRST_GROUP")
    });
    yinz.send_stdin(b"n");
    let extra = yinz.expect_pty_spawn();
    extra.output(b"TAB_MODE_PRESERVED");
    yinz.wait_until("group shortcut preserved Tab mode", |grid| {
        grid.contains("TAB_MODE_PRESERVED")
    });
    yinz.send_stdin(b"\x1b[108;4u");
    yinz.wait_until("second group restored", |grid| {
        grid.contains("SECOND_GROUP")
    });
    yinz.send_stdin(b"\x1b[104;4u");
    yinz.wait_until("last-used tab restored", |grid| {
        grid.contains("TAB_MODE_PRESERVED") && !grid.contains("FIRST_GROUP")
    });
    yinz.send_stdin(b"normal-mode");
    extra.wait_for_stdin("Normal mode preserved", |bytes| {
        bytes.ends_with(b"normal-mode")
    });
    yinz.quit();
}

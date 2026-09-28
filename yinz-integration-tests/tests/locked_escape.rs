#![cfg(unix)]

use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, start_yinz, TestRunner, TestSession,
    TERMINAL_SIZE,
};

fn lock(yinz: &TestSession) {
    yinz.send_stdin(&keys::ctrl('g'));
    yinz.wait_until("interface locked", |grid| {
        grid.contains("LOCK") && !grid.contains("PANE")
    });
}

#[test]
fn locked_escape_sends_ctrl_g_and_preserves_free_typing() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    lock(&yinz);

    // Each chord is a single input batch so scheduler delays between test
    // instructions cannot put its second key outside the 250ms window.
    yinz.send_stdin(b"\x07gplain g\x1bg\x10");
    let bytes = terminal.wait_for_stdin("quoted Ctrl+g and locked typing", |bytes| {
        bytes.ends_with(b"\x10")
    });
    assert_eq!(bytes, b"\x07plain g\x1bg\x10");
    // A second chord also exercises cancellation of the first timer.
    yinz.send_stdin(b"\x07g!");
    assert_eq!(
        terminal.wait_for_stdin("second quoted Ctrl+g", |bytes| bytes.ends_with(b"!")),
        b"\x07plain g\x1bg\x10\x07!"
    );
    yinz.quit();
}

#[test]
fn locked_escape_accepts_kitty_keys_but_writes_a_legacy_ctrl_g_byte() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    lock(&yinz);
    yinz.send_stdin(b"\x1b[103;5u\x1b[103u\x10");
    assert_eq!(
        terminal.wait_for_stdin("Kitty chord and locked Ctrl+p", |bytes| bytes
            .ends_with(b"\x10")),
        b"\x07\x10"
    );
    yinz.quit();
}

/// `locked_escape_timeout_ms 0` turns the prefix off: Ctrl+g unlocks at once, and there is
/// no way left to quote it into the pane.
#[test]
fn a_zero_timeout_unlocks_immediately_and_never_quotes_ctrl_g() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("locked_escape_timeout_ms 0")
        .start();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    lock(&yinz);
    yinz.send_stdin(b"\x07g!");
    yinz.wait_until("unlocked without waiting out a window", |grid| {
        grid.contains("PANE")
    });
    // The g and the ! arrive as ordinary input in Normal mode; no 0x07 is written.
    assert_eq!(
        terminal.wait_for_stdin("no quoted Ctrl+g", |bytes| bytes.ends_with(b"!")),
        b"g!"
    );
    yinz.quit();
}

#[test]
fn locked_escape_timeout_unlocks_without_sending_bytes() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    lock(&yinz);
    yinz.send_stdin(b"\x07");
    yinz.wait_until("timer unlocked without more input", |grid| {
        grid.contains("PANE")
    });
    assert!(terminal.stdin_bytes().is_empty());
    yinz.send_stdin(b"g!");
    assert_eq!(
        terminal.wait_for_stdin("late g is ordinary input", |bytes| bytes.ends_with(b"!")),
        b"g!"
    );
    yinz.quit();
}

#[test]
fn locked_escape_other_key_uses_normal_mode_bindings() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    lock(&yinz);
    yinz.send_stdin(b"\x07\x10");
    yinz.wait_until("Ctrl+p entered Pane mode after unlock", |grid| {
        grid.contains(" New ") && grid.contains(" Move ") && grid.contains(" Fullscreen ")
    });
    assert!(terminal.stdin_bytes().is_empty());
    yinz.quit();
}

#[test]
fn locked_escape_second_ctrl_g_relocks_before_following_input() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    lock(&yinz);
    yinz.send_stdin(b"\x07\x07\x10!");
    assert_eq!(
        terminal.wait_for_stdin("Ctrl+p passed through after relocking", |bytes| bytes
            .ends_with(b"!")),
        b"\x10!"
    );
    yinz.quit();
}

#[test]
fn locked_escape_pasted_g_does_not_complete_the_chord() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    lock(&yinz);
    yinz.send_stdin(b"\x07\x1b[200~g\x1b[201~");
    yinz.wait_until("paste unlocked the interface", |grid| grid.contains("PANE"));
    assert_eq!(
        terminal.wait_for_stdin("pasted g reached terminal", |bytes| bytes.contains(&b'g')),
        b"g"
    );
    yinz.quit();
}

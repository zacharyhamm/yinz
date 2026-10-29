#![cfg(unix)]

//! Keys that look like the start of a terminal reply (`ESC [`, `ESC ]`) must
//! reach the pane on the client's short idle flush, not after the one-second
//! guard reserved for a reply that is genuinely in progress.

use std::time::{Duration, Instant};

use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, start_yinz, TestSession,
};

const FAST_FLUSH_DELIVERY_CEILING: Duration = Duration::from_millis(700);

fn lock_interface(yinz: &TestSession) {
    yinz.send_stdin(&keys::ctrl('g'));
    yinz.wait_until("interface locked", |grid_snapshot| {
        grid_snapshot.contains("LOCK") && !grid_snapshot.contains("PANE")
    });
}

fn unlock_interface(yinz: &TestSession) {
    yinz.send_stdin(&keys::ctrl('g'));
    yinz.wait_until("interface unlocked", |grid_snapshot| {
        grid_snapshot.contains("PANE")
    });
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn a_bare_csi_introducer_reaches_the_pane_without_the_reply_guard() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    lock_interface(&yinz);

    let sent_at = Instant::now();
    yinz.send_stdin(b"\x1b[");

    let pane_stdin = terminal.wait_for_stdin("the bare Alt+[ reached the pane", |stdin_bytes| {
        contains_subslice(stdin_bytes, b"\x1b[")
    });
    let elapsed = sent_at.elapsed();
    assert_eq!(
        pane_stdin, b"\x1b[",
        "only the Alt+[ bytes may reach the pane"
    );
    assert!(
        elapsed < FAST_FLUSH_DELIVERY_CEILING,
        "Alt+[ must be released by the short idle flush, not the 1s reply guard: took {:?}",
        elapsed
    );

    unlock_interface(&yinz);
    yinz.quit();
}

#[test]
fn a_bare_osc_introducer_reaches_the_pane_without_the_reply_guard() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    lock_interface(&yinz);

    let sent_at = Instant::now();
    yinz.send_stdin(b"\x1b]");

    let pane_stdin = terminal.wait_for_stdin("the bare Alt+] reached the pane", |stdin_bytes| {
        contains_subslice(stdin_bytes, b"\x1b]")
    });
    let elapsed = sent_at.elapsed();
    assert_eq!(
        pane_stdin, b"\x1b]",
        "only the Alt+] bytes may reach the pane"
    );
    assert!(
        elapsed < FAST_FLUSH_DELIVERY_CEILING,
        "Alt+] must be released by the short idle flush, not the 1s reply guard: took {:?}",
        elapsed
    );

    unlock_interface(&yinz);
    yinz.quit();
}

#[test]
fn a_key_typed_after_a_bare_osc_introducer_is_not_swallowed() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    lock_interface(&yinz);

    yinz.send_stdin(b"\x1b]");
    // Wait for the idle flush to release the introducer before typing the
    // next key, so the key is typed after the introducer was classified.
    terminal.wait_for_stdin("the bare Alt+] reached the pane", |stdin_bytes| {
        contains_subslice(stdin_bytes, b"\x1b]")
    });
    let typed_at = Instant::now();
    yinz.send_stdin(b"z");

    let pane_stdin = terminal.wait_for_stdin(
        "the key typed after Alt+] reached the pane",
        |stdin_bytes| stdin_bytes.contains(&b'z'),
    );
    let elapsed = typed_at.elapsed();
    assert_eq!(
        pane_stdin, b"\x1b]z",
        "Alt+] and the key after it must arrive intact and separate"
    );
    assert!(
        elapsed < FAST_FLUSH_DELIVERY_CEILING,
        "the key after Alt+] must not be absorbed into the OSC accumulator: took {:?}",
        elapsed
    );

    unlock_interface(&yinz);
    yinz.quit();
}

#[test]
fn a_kitty_key_after_a_plain_key_in_the_same_read_is_not_swallowed() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();

    // The plain key and Kitty Ctrl-p deliberately share one stdin read. Ctrl-p
    // must still be decoded independently and switch Yinz into pane mode.
    yinz.send_stdin(b"x\x1b[112;5u");

    terminal.wait_for_stdin("the leading plain key reached the pane", |stdin_bytes| {
        stdin_bytes == b"x"
    });
    yinz.wait_until(
        "the trailing Kitty Ctrl-p entered pane mode",
        |grid_snapshot| {
            grid_snapshot.contains(" New ")
                && grid_snapshot.contains(" Move ")
                && grid_snapshot.contains(" Fullscreen ")
        },
    );

    yinz.quit();
}

#[test]
fn extended_function_keys_cross_ipc_without_injecting_unsupported_functional_text() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    terminal.output(b"\x1b[>1u\x1b[?u");
    let before = terminal.wait_for_stdin("the pane enabled Kitty keys", |bytes| {
        bytes.ends_with(b"\x1b[?1u")
    });

    let function_keys: String = (57376..=57398)
        .map(|code| format!("\x1b[{code};5u"))
        .collect();
    let sentinel = "\x1b[122;1u";
    // Volume-up and an unsupported Hyper modifier must not become private-use text.
    yinz.send_stdin(format!("{function_keys}\x1b[57438u\x1b[57445u{sentinel}").as_bytes());
    let received = terminal.wait_for_stdin(
        "all extended keys and the sentinel reached the pane",
        |bytes| bytes.ends_with(sentinel.as_bytes()),
    );
    assert_eq!(
        &received[before.len()..],
        format!("{function_keys}{sentinel}").as_bytes()
    );
    yinz.quit();
}

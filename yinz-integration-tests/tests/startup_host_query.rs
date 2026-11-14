#![cfg(unix)]

use std::thread::sleep;
use std::time::Duration;

use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, start_yinz, FakePtyHandle, LayoutInfo,
    TestRunner, TestSession, PROMPT, TERMINAL_SIZE,
};

const IDLE_BEYOND_SHORT_FLUSH: Duration = Duration::from_millis(150);

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

#[test]
fn fragmented_attach_reply_burst_does_not_leak_into_focused_pane() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    lock_interface(&yinz);

    yinz.send_stdin(b"\x1b]4;1;rgb:abcd");
    sleep(IDLE_BEYOND_SHORT_FLUSH);
    yinz.send_stdin(b"/ef01/2345\x1b\\");

    yinz.send_stdin(b"Z");
    let stdin = terminal.wait_for_stdin("sentinel keystroke reached the pane", |stdin_bytes| {
        stdin_bytes.contains(&b'Z')
    });

    assert!(
        !stdin.windows(4).any(|window| window == b"rgb:"),
        "host reply bytes leaked into the pane: {:?}",
        stdin
    );
    assert!(
        !stdin.contains(&b'/'),
        "host reply payload leaked into the pane: {:?}",
        stdin
    );
    assert!(
        !stdin.contains(&b';'),
        "host reply payload leaked into the pane: {:?}",
        stdin
    );

    unlock_interface(&yinz);
    yinz.quit();
}

#[test]
fn theme_query_from_pane_is_answered_when_host_mode_is_unknown() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    terminal.output(b"\x1b[?996n");

    terminal.wait_for_stdin("theme mode reply reached the pane", |stdin_bytes| {
        stdin_bytes
            .windows(9)
            .any(|window| window == b"\x1b[?997;1n")
    });

    yinz.quit();
}

#[test]
fn fragmented_function_key_arrives_intact() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    lock_interface(&yinz);

    yinz.send_stdin(b"\x1b[1;5");
    sleep(IDLE_BEYOND_SHORT_FLUSH);
    yinz.send_stdin(b"C");

    terminal.wait_for_stdin("ctrl-right reached the pane intact", |stdin_bytes| {
        stdin_bytes.windows(6).any(|window| window == b"\x1b[1;5C")
    });

    unlock_interface(&yinz);
    yinz.quit();
}

#[test]
fn normal_typing_is_not_delayed_or_duplicated() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    lock_interface(&yinz);

    yinz.send_stdin(b"hello");
    let stdin = terminal.wait_for_stdin("typed keys reached the pane", |stdin_bytes| {
        stdin_bytes.windows(5).any(|window| window == b"hello")
    });

    let occurrences = stdin
        .windows(5)
        .filter(|window| *window == b"hello")
        .count();
    assert_eq!(
        occurrences, 1,
        "typed keys must reach the pane exactly once: {:?}",
        stdin
    );

    unlock_interface(&yinz);
    yinz.quit();
}

/// What applications are told for `theme "nightfox"` (OSC 11, 10, 4;1 and CSI ?996n).
const NIGHTFOX: &str =
    "\x1b]11;rgb:1919/2323/3030\x07\x1b]10;rgb:cdcd/cece/cfcf\x07\x1b]4;1;rgb:c9c9/4f4f/6d6d\x07\x1b[?997;1n";
/// The same for `theme "dayfox"`.
const DAYFOX: &str =
    "\x1b]11;rgb:f6f6/f2f2/eeee\x07\x1b]10;rgb:3d3d/2b2b/5a5a\x07\x1b]4;1;rgb:a5a5/2222/2f2f\x07\x1b[?997;2n";

/// Ask the pane's terminal for its colors and appearance, as an application would, and
/// return the answers. The appearance reply comes last.
fn application_answers(shell: &FakePtyHandle) -> String {
    let before = shell.stdin_bytes().len();
    shell.output(b"\x1b]11;?\x07\x1b]10;?\x07\x1b]4;1;?\x07\x1b[?996n");
    let bytes = shell.wait_for_stdin("color and appearance answers", |bytes| {
        bytes[before..]
            .windows(7)
            .any(|window| window == b"\x1b[?997;")
    });
    String::from_utf8_lossy(&bytes[before..]).into_owned()
}

fn appearance_notifications(shell: &FakePtyHandle, mode: u8) -> usize {
    let needle = format!("\x1b[?997;{mode}n");
    shell
        .stdin_bytes()
        .windows(needle.len())
        .filter(|window| *window == needle.as_bytes())
        .count()
}

#[test]
fn application_baseline_ignores_conflicting_viewer_reports() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("theme \"nightfox\"")
        .start();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    // Querying apps use raw mode: echoing a reply would set an application color.
    terminal.disable_echo();
    terminal.output(b"\x1b[?2031h");
    let second = yinz.attach_client(TERMINAL_SIZE);
    second.wait_until("second client loaded", |grid_snapshot| {
        grid_snapshot.tab_bar_appears() && grid_snapshot.status_bar_appears()
    });
    lock_interface(&yinz);
    second.send_stdin(&keys::ctrl('g'));
    second.wait_until("second client locked", |grid_snapshot| {
        grid_snapshot.contains("LOCK") && !grid_snapshot.contains("PANE")
    });
    assert_eq!(application_answers(&terminal), NIGHTFOX);

    // Each viewer's host terminal reports opposite colors and appearance through the real
    // client host-reply parser. A keystroke behind each burst, through the same client,
    // shows the burst reached the session first.
    let light = b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b]10;rgb:0000/0000/0000\x1b\\\x1b]4;1;rgb:1111/1111/1111\x1b\\\x1b[?997;2n";
    let dark = b"\x1b]11;rgb:0101/0101/0101\x1b\\\x1b]10;rgb:fefe/fefe/fefe\x1b\\\x1b]4;1;rgb:2222/2222/2222\x1b\\\x1b[?997;1n";
    for (first, second_reports, sentinel) in [(light, dark, b'Y'), (dark, light, b'Z')] {
        yinz.send_stdin(first);
        yinz.send_stdin(&[sentinel]);
        second.send_stdin(second_reports);
        second.send_stdin(&[sentinel.to_ascii_lowercase()]);
        terminal.wait_for_stdin("both viewers' reports reached the session", |bytes| {
            bytes.contains(&sentinel) && bytes.contains(&sentinel.to_ascii_lowercase())
        });
        assert_eq!(application_answers(&terminal), NIGHTFOX);
    }
    let stdin = terminal.stdin_bytes();
    assert!(
        !stdin
            .windows(8)
            .any(|window| window == b"rgb:ffff" || window == b"rgb:0101"),
        "a viewer's host colors reached the pane: {:?}",
        String::from_utf8_lossy(&stdin)
    );
    // Only the three CSI ?996n answers; no viewer report was relayed into the PTY.
    assert_eq!(appearance_notifications(&terminal, 1), 3);
    assert_eq!(appearance_notifications(&terminal, 2), 0);

    second.quit();
    unlock_interface(&yinz);
    yinz.quit();
}

#[test]
fn application_baseline_ignores_second_client_theme_options() {
    let directory = tempfile::tempdir().unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(
            r#"
            load_plugins clear-defaults=true {}
            pane_animations false
            theme "nightfox"
            pane_frame_style "full"
            "#,
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    let initial = yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    let frame = initial.cell_foreground(0, 0);
    shell.disable_echo();
    shell.output(b"\x1b[?2031h");
    assert_eq!(application_answers(&shell), NIGHTFOX);

    // A second viewer brings its own theme and light hue. They restyle the shared chrome
    // (per-viewer chrome is later work) but not what applications are told.
    let second = yinz.attach_client_with_config_options(
        TERMINAL_SIZE,
        yinz_types::input::options::Options {
            theme: Some("dayfox".into()),
            explicit_theme_hue: Some(yinz_types::style::ThemeHue::Light),
            ..Default::default()
        },
    );
    yinz.wait_until(
        "the second viewer's theme restyles the chrome",
        |snapshot| snapshot.contains("$") && snapshot.cell_foreground(0, 0) != frame,
    );
    assert_eq!(application_answers(&shell), NIGHTFOX);
    assert_eq!(appearance_notifications(&shell, 2), 0);

    // A session-wide change is session policy and moves the baseline, announced once.
    assert_eq!(
        yinz.run_cli_action(CliAction::Reconfigure {
            kdl: Some("theme \"dayfox\"".into()),
            file: None,
            write: false,
        }),
        0
    );
    shell.wait_for_stdin("the session-wide theme is announced", |bytes| {
        bytes.windows(9).any(|window| window == b"\x1b[?997;2n")
    });
    assert_eq!(application_answers(&shell), DAYFOX);
    assert_eq!(appearance_notifications(&shell, 2), 2);

    // A saved unrelated change while the second viewer is attached. The disk reload that
    // follows makes the disk config the session's theme again, replacing the unsaved
    // `theme "dayfox"` just as it resets every client's unsaved settings; every reload of
    // this write loads the same file, so the outcome does not depend on how many arrive.
    // (Which client's config a batch save builds on depends on hashing, so whether a
    // client's own options could leak is pinned by the server tests instead.)
    let reloads = yinz
        .received_server_messages()
        .iter()
        .filter(|message| *message == "ConfigFileUpdated")
        .count();
    let dark_notifications = appearance_notifications(&shell, 1);
    assert_eq!(
        yinz.run_cli_action(CliAction::Reconfigure {
            kdl: Some("pane_frames false".into()),
            file: None,
            write: true,
        }),
        0
    );
    yinz.wait_until("pane frames are gone", |snapshot| {
        snapshot.contains("$") && !snapshot.contains("┌")
    });
    yinz.wait_for_server_messages("the managed-file watcher reloaded the config", |messages| {
        messages
            .iter()
            .filter(|message| *message == "ConfigFileUpdated")
            .count()
            > reloads
    });
    shell.wait_for_stdin("the disk theme is announced", |bytes| {
        bytes
            .windows(9)
            .filter(|window| *window == b"\x1b[?997;1n")
            .count()
            > dark_notifications
    });
    assert_eq!(application_answers(&shell), NIGHTFOX);
    assert_eq!(appearance_notifications(&shell, 2), 2);

    second.quit();
    yinz.quit();
}

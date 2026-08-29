#![cfg(unix)]

//! The about plugin's release notes: the "what's new" list for the running version, and
//! every page it links to, rendered in the plugin's own 20x90 pane.

use insta::assert_snapshot;
use zellij_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, normalized, GridSnapshot, TestRunner,
    TestSession, TERMINAL_SIZE,
};

/// Opens the about plugin the way the server does on a version change: floating, with
/// `is_release_notes` set so the main screen greets the new version.
const ABOUT_CONFIG: &str = r#"keybinds {
  normal {
    bind "Alt a" {
      LaunchOrFocusPlugin "zellij:about" {
        floating true
        is_release_notes "true"
      }
    }
  }
}
"#;

const DOWN_ARROW: &[u8] = b"\x1b[B";

/// The "what's new" menu, top to bottom. The "Update Keybindings" page is not in it: that
/// page sits behind `u`, offered only when the config lacks one of the new binds, and the
/// default config binds them all.
const RELEASE_NOTE_PAGES: [&str; 6] = [
    "Nested Sessions",
    "Kitty Graphics Protocol",
    "Scroll By Command",
    "Mobile Web UI",
    "New UI",
    "Focus Last Pane and Fullscreen Floating Panes",
];

/// The main screen, laid out in the plugin's final pane size: the plugin renders once
/// before it has resized its own pane, and that first frame truncates the links.
fn main_screen_is_settled(grid: &GridSnapshot) -> bool {
    grid.contains("Hi there, welcome to Zellij")
        && grid.contains("Focus Last Pane and Fullscreen Floating Panes")
        && grid.contains("https://github.com/sponsors/imsnif")
}

fn open_release_notes() -> (TestSession, GridSnapshot) {
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(ABOUT_CONFIG)
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);
    zellij.send_stdin(&keys::alt('a'));
    let grid = zellij.wait_until("release notes main screen rendered", main_screen_is_settled);
    (zellij, grid)
}

fn open_page(zellij: &TestSession, index: usize, title: &str) -> GridSnapshot {
    for _ in 0..=index {
        zellij.send_stdin(DOWN_ARROW);
    }
    zellij.send_stdin(&keys::ENTER);
    zellij.wait_until(&format!("{} page rendered", title), |grid| {
        grid.contains(title) && !grid.contains("Hi there, welcome to Zellij")
    })
}

fn go_back_to_main_screen(zellij: &TestSession) {
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until(
        "release notes main screen rendered again",
        main_screen_is_settled,
    );
}

#[test]
fn release_notes_greet_the_running_version_with_the_whats_new_list() {
    let (mut zellij, grid) = open_release_notes();
    assert_snapshot!(normalized(&grid));
    zellij.quit();
}

#[test]
fn every_release_note_page_renders_and_returns_to_the_main_screen() {
    let (mut zellij, _main_screen) = open_release_notes();
    let mut rendered_pages = Vec::new();
    for (index, title) in RELEASE_NOTE_PAGES.iter().enumerate() {
        let grid = open_page(&zellij, index, title);
        rendered_pages.push(format!("=== {}\n{}", title, normalized(&grid)));
        go_back_to_main_screen(&zellij);
    }
    assert_snapshot!(rendered_pages.join("\n"));
    zellij.quit();
}

#![cfg(unix)]

//! The about plugin's release notes: the "what's new" list for the running version, and
//! every page it links to, rendered in the plugin's own 20x90 pane.

use insta::assert_snapshot;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, normalized, GridSnapshot, TestRunner,
    TestSession, TERMINAL_SIZE,
};

/// Opens the about plugin the way the server does on a version change: floating, with
/// `is_release_notes` set so the main screen greets the new version.
const ABOUT_CONFIG: &str = r#"keybinds {
  normal {
    bind "Alt a" {
      LaunchOrFocusPlugin "yinz:about" {
        floating true
        is_release_notes "true"
      }
    }
  }
}
"#;

const DOWN_ARROW: &[u8] = b"\x1b[B";

const GREETING: &str = "Hi there, welcome to Yinz";

/// The width the plugin gives its own pane (`UI_COLUMNS` in
/// `default-plugins/about/src/main.rs`). Its first render is laid out for the default
/// floating pane: shown before the resize, that is the same screen centred in a narrower
/// frame, and the width is all that tells the two apart.
const ABOUT_PANE_COLUMNS: usize = 90;

/// The about pane's width, borders included, and the text between its side borders on
/// each row, found from the side borders around the greeting.
fn about_pane(grid: &GridSnapshot) -> Option<(usize, Vec<String>)> {
    let lines: Vec<Vec<char>> = grid
        .text
        .lines()
        .map(|line| line.chars().collect())
        .collect();
    let (greeting_row, greeting_column) =
        grid.text.lines().enumerate().find_map(|(row, line)| {
            let byte = line.find(GREETING)?;
            Some((row, line[..byte].chars().count()))
        })?;
    let greeting_line = &lines[greeting_row];
    let left = greeting_line[..greeting_column]
        .iter()
        .rposition(|character| *character == '│')?;
    let right = greeting_column
        + greeting_line[greeting_column..]
            .iter()
            .position(|character| *character == '│')?;
    let is_inside =
        |line: &Vec<char>| line.get(left) == Some(&'│') && line.get(right) == Some(&'│');
    let top = lines[..greeting_row]
        .iter()
        .rposition(|line| !is_inside(line))
        .map_or(0, |row| row + 1);
    let rows = lines[top..]
        .iter()
        .take_while(|line| is_inside(line))
        .map(|line| line[left + 1..right].iter().collect())
        .collect();
    Some((right - left + 1, rows))
}

/// The numbered rows under "What's new?", padding included.
fn whats_new_rows(pane_rows: &[String]) -> Vec<&str> {
    pane_rows
        .iter()
        .skip_while(|row| !row.contains("What's new?"))
        .skip(1)
        .take_while(|row| {
            row.trim_start()
                .split_once(". ")
                .is_some_and(|(number, _)| number.parse::<usize>().is_ok())
        })
        .map(String::as_str)
        .collect()
}

/// Whether the page sits centred between the pane's borders, the plugin rounding the left
/// margin down. The help line doesn't count: it is printed flush against the left border,
/// outside the centred block. The first render, still showing once the pane has grown,
/// sits off to the left.
fn is_centred(pane_rows: &[String]) -> bool {
    let content: Vec<&str> = pane_rows
        .iter()
        .map(String::as_str)
        .filter(|row| row.starts_with(' ') && !row.trim_start().is_empty())
        .collect();
    let left = content
        .iter()
        .map(|row| row.len() - row.trim_start().len())
        .min();
    let right = content
        .iter()
        .map(|row| row.len() - row.trim_end().len())
        .min();
    matches!((left, right), (Some(left), Some(right)) if right == left || right == left + 1)
}

/// The main screen, laid out in the pane the plugin gives itself.
fn main_screen_is_settled(grid: &GridSnapshot) -> bool {
    about_pane(grid).is_some_and(|(width, rows)| width == ABOUT_PANE_COLUMNS && is_centred(&rows))
}

/// The "what's new" menu, top to bottom, as the main screen lists it. The "Update
/// Keybindings" page is not in it: that page sits behind `u`, offered only when the config
/// lacks one of the new binds, and the default config binds them all.
fn release_note_pages(main_screen: &GridSnapshot) -> Vec<String> {
    let (_, rows) = about_pane(main_screen).expect("the main screen is in the about pane");
    whats_new_rows(&rows)
        .iter()
        .filter_map(|row| row.trim().split_once(". "))
        .map(|(_, title)| title.to_owned())
        .collect()
}

fn open_release_notes() -> (TestSession, GridSnapshot) {
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(ABOUT_CONFIG)
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    yinz.send_stdin(&keys::alt('a'));
    let grid = yinz.wait_until("release notes main screen rendered", main_screen_is_settled);
    (yinz, grid)
}

fn open_page(yinz: &TestSession, index: usize, title: &str) -> GridSnapshot {
    for _ in 0..=index {
        yinz.send_stdin(DOWN_ARROW);
    }
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until(&format!("{} page rendered", title), |grid| {
        grid.contains(title) && !grid.contains(GREETING)
    })
}

fn go_back_to_main_screen(yinz: &TestSession) {
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until(
        "release notes main screen rendered again",
        main_screen_is_settled,
    );
}

#[test]
fn release_notes_greet_the_running_version_with_the_whats_new_list() {
    let (mut yinz, grid) = open_release_notes();
    assert_snapshot!(normalized(&grid));
    yinz.quit();
}

#[test]
fn every_release_note_page_renders_and_returns_to_the_main_screen() {
    let (mut yinz, main_screen) = open_release_notes();
    let mut rendered_pages = Vec::new();
    for (index, title) in release_note_pages(&main_screen).iter().enumerate() {
        let grid = open_page(&yinz, index, title);
        rendered_pages.push(format!("=== {}\n{}", title, normalized(&grid)));
        go_back_to_main_screen(&yinz);
    }
    assert_snapshot!(rendered_pages.join("\n"));
    yinz.quit();
}

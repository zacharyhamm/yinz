#![cfg(unix)]

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::engine::Engine as _;
use yinz_integration_tests::{
    keys, FakePtyHandle, GridSnapshot, TestRunner, TestSession, TERMINAL_SIZE,
};

const NAVIGATION_CONFIG: &str = concat!(
    "mouse_mode true\n",
    "copy_clipboard \"system\"\n",
    "keybinds {\n",
    " normal {\n",
    "  bind \"Ctrl y\" { ScrollToPreviousPrompt; }\n",
    "  bind \"Ctrl u\" { ScrollToNextPrompt; }\n",
    "  bind \"Ctrl v\" { SelectCommandAtScrollPosition; }\n",
    "  bind \"Ctrl x\" { CopyLastCommandOutput; }\n",
    " }\n",
    " scroll {\n",
    "  bind \"Ctrl y\" { Copy; }\n",
    " }\n",
    "}\n",
);

const COPY_CONFIG: &str = concat!("mouse_mode true\n", "copy_clipboard \"system\"\n");

const MOUSE_CONFIG: &str = concat!("mouse_mode true\n", "advanced_mouse_actions true\n");

const MARKED_COMMAND: &[u8] = b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\x1b]133;C\x07\r\nhello world\r\nsecond line\x1b]133;D;0\x07\r\n\x1b]133;A\x07$ ";

const COMMAND_AND_OUTPUT: &str = "echo hi\nhello world\nsecond line";
const LAST_COMMAND_OUTPUT: &str = "hello world\nsecond line";

const ALT_WHEEL_UP: u8 = 72;
const ALT_WHEEL_DOWN: u8 = 73;

const SCROLL_MODE_HINT: &str = "PgDn|PgUp";

fn start_with_config(config: &str) -> TestSession {
    TestRunner::new(TERMINAL_SIZE).with_config(config).start()
}

fn claim_terminal_with(yinz: &TestSession, pane_output: &[u8], last_line: &str) -> FakePtyHandle {
    let terminal = yinz.expect_pty_spawn();
    terminal.output(pane_output);
    yinz.wait_until("marked pane output rendered", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.contains(last_line)
    });
    terminal
}

fn marked_commands(count: usize) -> Vec<u8> {
    let mut content = String::new();
    for command_index in 0..count {
        content.push_str(&format!(
            "\u{1b}]133;A\u{7}$ \u{1b}]133;B\u{7}cmd{:02}\u{1b}]133;C\u{7}\r\nout{:02}\u{1b}]133;D;0\u{7}\r\n",
            command_index, command_index
        ));
    }
    content.into_bytes()
}

fn top_content_line(grid_snapshot: &GridSnapshot) -> String {
    grid_snapshot
        .lines()
        .get(2) // The tab bar and pane title precede terminal content.
        .map(|line| line.trim_end().to_owned())
        .unwrap_or_default()
}

fn enter_scroll_mode(yinz: &TestSession) {
    yinz.send_stdin(&keys::ctrl('s'));
    yinz.wait_until("scroll mode active", |grid_snapshot| {
        grid_snapshot.contains(SCROLL_MODE_HINT)
    });
}

fn sgr_mouse_report(column: usize, line: usize, button: u8) -> Vec<u8> {
    format!("\u{1b}[<{};{};{}M", button, column, line).into_bytes()
}

fn osc52_copy_of(text: &str) -> Vec<u8> {
    format!("\u{1b}]52;c;{}\u{1b}\\", BASE64_STANDARD.encode(text)).into_bytes()
}

fn occurrences_of(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

#[test]
fn bracket_keys_jump_between_prompts_in_scroll_mode() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE).start();
    claim_terminal_with(&yinz, &marked_commands(12), "out11");
    enter_scroll_mode(&yinz);

    let before_jump = top_content_line(&yinz.snapshot());

    yinz.send_stdin(&keys::key('['));
    let after_first_jump = yinz.wait_until("jumped to the previous prompt", |grid_snapshot| {
        let top_line = top_content_line(grid_snapshot);
        top_line.starts_with("$ cmd") && top_line != before_jump
    });
    let first_prompt = top_content_line(&after_first_jump);

    yinz.send_stdin(&keys::key('['));
    yinz.wait_until("jumped to an earlier prompt", |grid_snapshot| {
        let top_line = top_content_line(grid_snapshot);
        top_line.starts_with("$ cmd") && top_line != first_prompt
    });

    yinz.send_stdin(&keys::key(']'));
    yinz.wait_until("jumped forward to the later prompt", |grid_snapshot| {
        top_content_line(grid_snapshot) == first_prompt
    });

    yinz.quit();
}

#[test]
fn a_prompt_jump_from_normal_mode_does_not_enter_scroll_mode() {
    let mut yinz = start_with_config(NAVIGATION_CONFIG);
    claim_terminal_with(&yinz, &marked_commands(12), "out11");

    let before_jump = top_content_line(&yinz.snapshot());

    yinz.send_stdin(&keys::ctrl('y'));
    let after_jump = yinz.wait_until(
        "jumped to the previous prompt from normal mode",
        |grid_snapshot| {
            let top_line = top_content_line(grid_snapshot);
            top_line.starts_with("$ cmd") && top_line != before_jump
        },
    );

    assert!(
        !after_jump.contains(SCROLL_MODE_HINT),
        "a prompt jump outside scroll mode must not switch modes"
    );

    yinz.quit();
}

#[test]
fn selecting_the_command_at_the_scroll_position_composes_with_copy() {
    let mut yinz = start_with_config(NAVIGATION_CONFIG);
    claim_terminal_with(&yinz, MARKED_COMMAND, "second line");
    enter_scroll_mode(&yinz);

    yinz.send_stdin(&keys::key('m'));
    yinz.send_stdin(&keys::ctrl('y'));

    let expected_copy = osc52_copy_of(COMMAND_AND_OUTPUT);
    yinz.wait_until_raw_output("the selected command and its output were copied", |bytes| {
        occurrences_of(bytes, &expected_copy) >= 1
    });

    yinz.quit();
}

#[test]
fn keyboard_selection_copies_and_returns_to_normal_mode() {
    let mut yinz = start_with_config(COPY_CONFIG);
    claim_terminal_with(&yinz, b"select", "select");

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('v'));
    for _ in 0..6 {
        yinz.send_stdin(&keys::key('h'));
    }
    yinz.send_stdin(&keys::ENTER);

    let expected_copy = osc52_copy_of("select");
    yinz.wait_until_raw_output("the keyboard selection was copied", |bytes| {
        occurrences_of(bytes, &expected_copy) >= 1
    });
    yinz.wait_until("selection mode was left after the copy", |grid_snapshot| {
        !grid_snapshot.contains("Move selection")
    });

    yinz.quit();
}

#[test]
fn swapping_the_selection_ends_moves_the_start_of_the_keyboard_selection() {
    let mut yinz = start_with_config(COPY_CONFIG);
    claim_terminal_with(&yinz, b"abcdef", "abcdef");

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('v'));
    // the selection is anchored at the cursor, past "abcdef", and grows leftwards over "cdef"
    for _ in 0..4 {
        yinz.send_stdin(&keys::key('h'));
    }
    // now move the anchor instead: without the swap these two keys would select "abcdef"
    yinz.send_stdin(&keys::key('o'));
    for _ in 0..2 {
        yinz.send_stdin(&keys::key('h'));
    }
    yinz.send_stdin(&keys::ENTER);

    let expected_copy = osc52_copy_of("cd");
    yinz.wait_until_raw_output("the selection with a moved start was copied", |bytes| {
        occurrences_of(bytes, &expected_copy) >= 1
    });

    yinz.quit();
}

#[test]
fn copying_the_last_command_output_flashes_it_and_leaves_scroll_mode() {
    let mut yinz = start_with_config(COPY_CONFIG);
    claim_terminal_with(&yinz, MARKED_COMMAND, "second line");

    let before_copy = yinz.snapshot();
    let output_row = before_copy
        .row_of_line("hello world")
        .expect("the command output must be rendered");
    let steady_foreground = before_copy.cell_foreground(0, output_row);

    enter_scroll_mode(&yinz);
    yinz.send_stdin(&keys::key('c'));

    let expected_copy = osc52_copy_of(LAST_COMMAND_OUTPUT);
    yinz.wait_until_raw_output("the last command output was copied", |bytes| {
        occurrences_of(bytes, &expected_copy) >= 1
    });

    yinz.wait_until("the copied output is highlighted", |grid_snapshot| {
        grid_snapshot.cell_foreground(0, output_row) != steady_foreground
    });
    yinz.wait_until("scroll mode was left after the copy", |grid_snapshot| {
        !grid_snapshot.contains(SCROLL_MODE_HINT)
    });
    yinz.wait_until("the highlight is cleared again", |grid_snapshot| {
        grid_snapshot.cell_foreground(0, output_row) == steady_foreground
    });

    yinz.quit();
}

#[test]
fn alt_wheel_jumps_between_prompts() {
    let mut yinz = start_with_config(MOUSE_CONFIG);
    claim_terminal_with(&yinz, &marked_commands(12), "out11");

    let before_jump = top_content_line(&yinz.snapshot());

    yinz.send_stdin(&sgr_mouse_report(20, 5, ALT_WHEEL_UP));
    yinz.wait_until(
        "alt wheel up jumped to the previous prompt",
        |grid_snapshot| {
            let top_line = top_content_line(grid_snapshot);
            top_line.starts_with("$ cmd") && top_line != before_jump
        },
    );

    yinz.send_stdin(&sgr_mouse_report(20, 5, ALT_WHEEL_DOWN));
    yinz.wait_until("alt wheel down jumped forward again", |grid_snapshot| {
        top_content_line(grid_snapshot) == before_jump
    });

    yinz.quit();
}

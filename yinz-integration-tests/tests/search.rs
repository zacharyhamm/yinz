#![cfg(unix)]

use insta::assert_snapshot;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, normalized, start_yinz, FakePtyHandle,
    TestRunner, TestSession, TERMINAL_SIZE,
};

fn start_yinz_with_full_frames() -> TestSession {
    TestRunner::new(TERMINAL_SIZE)
        .with_config("pane_frame_style \"full\"")
        .start()
}

fn enter_search_on_visible_match_with_full_frames(yinz: &TestSession) {
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"NEEDLE on screen\r\n");
    yinz.wait_until("match and full-frame bars visible", |grid_snapshot| {
        grid_snapshot.contains("NEEDLE on screen")
            && grid_snapshot.contains(" Tab #1")
            && grid_snapshot.status_bar_appears()
    });
    enter_search_for_needle(yinz);
}

fn fill_pane_with_two_matches(yinz: &TestSession) -> FakePtyHandle {
    let terminal = claim_first_terminal_and_wait_for_prompt(yinz);
    for line in 1..=40 {
        let content = match line {
            5 => "line05 NEEDLE upper\r\n".to_string(),
            35 => "line35 NEEDLE lower\r\n".to_string(),
            other => format!("line{:02}\r\n", other),
        };
        terminal.output(content.as_bytes());
    }
    yinz.wait_until(
        "pane filled with the lower match visible",
        |grid_snapshot| {
            grid_snapshot.contains("NEEDLE lower") && !grid_snapshot.contains("NEEDLE upper")
        },
    );
    terminal
}

fn enter_search_for_needle(yinz: &TestSession) {
    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::key('s'));
    yinz.wait_until("search input mode active", |grid_snapshot| {
        grid_snapshot.contains("ENTERING SEARCH TERM")
    });
    yinz.send_stdin(b"NEEDLE");
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("search navigation mode active", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp")
    });
}

#[test]
fn search_input_jumps_to_a_match() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    for line in 1..=40 {
        let content = if line == 5 {
            "line05 NEEDLE\r\n".to_string()
        } else {
            format!("line{:02}\r\n", line)
        };
        terminal.output(content.as_bytes());
    }
    yinz.wait_until("upper match initially off screen", |grid_snapshot| {
        grid_snapshot.contains("line40") && !grid_snapshot.contains("line05")
    });

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::key('s'));
    yinz.wait_until("search input mode active", |grid_snapshot| {
        grid_snapshot.contains("ENTERING SEARCH TERM")
    });
    yinz.send_stdin(b"NEEDLE");

    let grid_snapshot = yinz.wait_until("scrolled to the typed match", |grid_snapshot| {
        grid_snapshot.contains("line05")
            && grid_snapshot.contains("ENTERING SEARCH TERM")
            && grid_snapshot.contains("SCROLL 15/19")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn search_up_to_previous_match() {
    let mut yinz = start_yinz();
    fill_pane_with_two_matches(&yinz);
    enter_search_for_needle(&yinz);

    yinz.send_stdin(&keys::key('p'));
    yinz.send_stdin(&keys::key('p'));

    let grid_snapshot = yinz.wait_until("upper match revealed by searching up", |grid_snapshot| {
        grid_snapshot.contains("NEEDLE upper")
            && grid_snapshot.contains("PgDn|PgUp")
            && grid_snapshot.contains("SCROLL 15/19")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn search_down_to_next_match() {
    let mut yinz = start_yinz();
    fill_pane_with_two_matches(&yinz);
    enter_search_for_needle(&yinz);

    yinz.send_stdin(&keys::key('p'));
    yinz.send_stdin(&keys::key('p'));
    yinz.wait_until("upper match revealed by searching up", |grid_snapshot| {
        grid_snapshot.contains("NEEDLE upper") && grid_snapshot.contains("SCROLL 15/19")
    });

    yinz.send_stdin(&keys::key('n'));

    let grid_snapshot =
        yinz.wait_until("lower match revealed by searching down", |grid_snapshot| {
            grid_snapshot.contains("NEEDLE lower")
                && !grid_snapshot.contains("NEEDLE upper")
                && grid_snapshot.contains("SCROLL 6/19")
                && grid_snapshot.contains("PgDn|PgUp")
        });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn search_toggle_case_sensitivity() {
    let mut yinz = start_yinz_with_full_frames();
    enter_search_on_visible_match_with_full_frames(&yinz);

    yinz.send_stdin(&keys::key('c'));

    let grid_snapshot = yinz.wait_until("case sensitivity toggled on", |grid_snapshot| {
        grid_snapshot.contains("[c]")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn search_toggle_wrap() {
    let mut yinz = start_yinz_with_full_frames();
    enter_search_on_visible_match_with_full_frames(&yinz);

    yinz.send_stdin(&keys::key('w'));

    let grid_snapshot = yinz.wait_until("wrap toggled on", |grid_snapshot| {
        grid_snapshot.contains("[w]")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn search_toggle_whole_word() {
    let mut yinz = start_yinz_with_full_frames();
    enter_search_on_visible_match_with_full_frames(&yinz);

    yinz.send_stdin(&keys::key('o'));

    let grid_snapshot = yinz.wait_until("whole word toggled on", |grid_snapshot| {
        grid_snapshot.contains("[o]")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

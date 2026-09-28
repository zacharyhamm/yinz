#![cfg(unix)]

use insta::assert_snapshot;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, normalized,
    split_down_and_wait_for_prompt, split_right_and_wait_for_prompt, start_yinz, FakePtyHandle,
    GridSnapshot, TestSession,
};

fn focus_left_pane(yinz: &TestSession) {
    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('h'));
    yinz.wait_until("focus moved to the left pane", |grid_snapshot| {
        grid_snapshot.cursor_is_at(col(2).row(2))
    });
}

fn focus_upper_pane(yinz: &TestSession) {
    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('k'));
    yinz.wait_until("focus moved to the upper pane", |grid_snapshot| {
        grid_snapshot.cursor_is_at(col(2).row(2))
    });
}

fn resize_in_resize_mode(yinz: &TestSession, key: &[u8]) {
    yinz.send_stdin(&keys::ctrl('n'));
    yinz.send_stdin(key);
    yinz.send_stdin(&keys::ENTER);
}

fn width(terminal: &FakePtyHandle, what: &str) -> u16 {
    terminal.wait_for_size(what, |_, _| true).0
}

fn height(terminal: &FakePtyHandle, what: &str) -> u16 {
    terminal.wait_for_size(what, |_, _| true).1
}

fn pane_two_header_column(grid_snapshot: &GridSnapshot) -> Option<usize> {
    grid_snapshot.lines().into_iter().find_map(|line| {
        line.find("Pane #2")
            .map(|byte_index| line[..byte_index].chars().count())
    })
}

fn pane_two_header_row(grid_snapshot: &GridSnapshot) -> Option<usize> {
    grid_snapshot
        .lines()
        .into_iter()
        .position(|line| line.contains("Pane #2"))
}

fn vertical_boundary_column(yinz: &TestSession) -> usize {
    pane_two_header_column(&yinz.snapshot()).expect("vertical pane boundary rendered")
}

fn horizontal_boundary_row(yinz: &TestSession) -> usize {
    pane_two_header_row(&yinz.snapshot()).expect("horizontal pane boundary rendered")
}

#[test]
fn resize_increase_right() {
    let mut yinz = start_yinz();
    let left_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);
    focus_left_pane(&yinz);
    let initial_cols = width(&left_terminal, "left terminal initial width");
    let initial_boundary = vertical_boundary_column(&yinz);

    resize_in_resize_mode(&yinz, &keys::key('l'));

    left_terminal.wait_for_size("left terminal grew rightward", |cols, _| {
        cols > initial_cols
    });
    let grid_snapshot = yinz.wait_until("pane boundary moved right", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_column(grid_snapshot).is_some_and(|column| column > initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_decrease_left() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);
    let initial_cols = width(&right_terminal, "right terminal initial width");
    let initial_boundary = vertical_boundary_column(&yinz);

    resize_in_resize_mode(&yinz, &keys::key('H'));

    right_terminal.wait_for_size("right terminal shrank", |cols, _| cols < initial_cols);
    let grid_snapshot = yinz.wait_until("pane boundary moved right", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_column(grid_snapshot).is_some_and(|column| column > initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_decrease_right() {
    let mut yinz = start_yinz();
    let left_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);
    focus_left_pane(&yinz);
    let initial_cols = width(&left_terminal, "left terminal initial width");
    let initial_boundary = vertical_boundary_column(&yinz);

    resize_in_resize_mode(&yinz, &keys::key('L'));

    left_terminal.wait_for_size("left terminal shrank", |cols, _| cols < initial_cols);
    let grid_snapshot = yinz.wait_until("pane boundary moved left", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_column(grid_snapshot).is_some_and(|column| column < initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_increase_down() {
    let mut yinz = start_yinz();
    let upper_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    split_down_and_wait_for_prompt(&yinz);
    focus_upper_pane(&yinz);
    let initial_rows = height(&upper_terminal, "upper terminal initial height");
    let initial_boundary = horizontal_boundary_row(&yinz);

    resize_in_resize_mode(&yinz, &keys::key('j'));

    upper_terminal.wait_for_size("upper terminal grew downward", |_, rows| {
        rows > initial_rows
    });
    let grid_snapshot = yinz.wait_until("pane boundary moved down", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_row(grid_snapshot).is_some_and(|row| row > initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_increase_up() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let lower_terminal = split_down_and_wait_for_prompt(&yinz);
    let initial_rows = height(&lower_terminal, "lower terminal initial height");
    let initial_boundary = horizontal_boundary_row(&yinz);

    resize_in_resize_mode(&yinz, &keys::key('k'));

    lower_terminal.wait_for_size("lower terminal grew upward", |_, rows| rows > initial_rows);
    let grid_snapshot = yinz.wait_until("pane boundary moved up", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_row(grid_snapshot).is_some_and(|row| row < initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_decrease_down() {
    let mut yinz = start_yinz();
    let upper_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    split_down_and_wait_for_prompt(&yinz);
    focus_upper_pane(&yinz);
    let initial_rows = height(&upper_terminal, "upper terminal initial height");
    let initial_boundary = horizontal_boundary_row(&yinz);

    resize_in_resize_mode(&yinz, &keys::key('J'));

    upper_terminal.wait_for_size("upper terminal shrank", |_, rows| rows < initial_rows);
    let grid_snapshot = yinz.wait_until("pane boundary moved up", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_row(grid_snapshot).is_some_and(|row| row < initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_decrease_up() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let lower_terminal = split_down_and_wait_for_prompt(&yinz);
    let initial_rows = height(&lower_terminal, "lower terminal initial height");
    let initial_boundary = horizontal_boundary_row(&yinz);

    resize_in_resize_mode(&yinz, &keys::key('K'));

    lower_terminal.wait_for_size("lower terminal shrank", |_, rows| rows < initial_rows);
    let grid_snapshot = yinz.wait_until("pane boundary moved down", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_row(grid_snapshot).is_some_and(|row| row > initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_increase_in_resize_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);
    let initial_cols = width(&right_terminal, "right terminal initial width");
    let initial_boundary = vertical_boundary_column(&yinz);

    resize_in_resize_mode(&yinz, &keys::key('='));

    right_terminal.wait_for_size("right terminal grew", |cols, _| cols > initial_cols);
    let grid_snapshot = yinz.wait_until("pane boundary moved left", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_column(grid_snapshot).is_some_and(|column| column < initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_decrease_in_resize_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);
    let initial_cols = width(&right_terminal, "right terminal initial width");
    let initial_boundary = vertical_boundary_column(&yinz);

    resize_in_resize_mode(&yinz, &keys::key('-'));

    right_terminal.wait_for_size("right terminal shrank", |cols, _| cols < initial_cols);
    let grid_snapshot = yinz.wait_until("pane boundary moved right", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_column(grid_snapshot).is_some_and(|column| column > initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_increase_in_normal_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);
    let initial_cols = width(&right_terminal, "right terminal initial width");
    let initial_boundary = vertical_boundary_column(&yinz);

    yinz.send_stdin(&keys::alt('='));

    right_terminal.wait_for_size("right terminal grew", |cols, _| cols > initial_cols);
    let grid_snapshot = yinz.wait_until("pane boundary moved left", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_column(grid_snapshot).is_some_and(|column| column < initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_decrease_in_normal_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);
    let initial_cols = width(&right_terminal, "right terminal initial width");
    let initial_boundary = vertical_boundary_column(&yinz);

    yinz.send_stdin(&keys::alt('-'));

    right_terminal.wait_for_size("right terminal shrank", |cols, _| cols < initial_cols);
    let grid_snapshot = yinz.wait_until("pane boundary moved right", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && pane_two_header_column(grid_snapshot).is_some_and(|column| column > initial_boundary)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

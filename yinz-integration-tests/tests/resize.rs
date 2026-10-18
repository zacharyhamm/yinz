#![cfg(unix)]

use insta::assert_snapshot;
use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, normalized,
    split_down_and_wait_for_prompt, split_right_and_wait_for_prompt, start_yinz, FakePtyHandle,
    GridSnapshot, LayoutInfo, Size, TestRunner, TestSession,
};
use yinz_types::geometry::{Direction, Resize};

fn ctrl_mouse(yinz: &TestSession, x: usize, y: usize, button: u8, release: bool) {
    let suffix = if release { 'm' } else { 'M' };
    yinz.send_stdin(format!("\x1b[<{};{};{}{suffix}", button, x + 1, y + 1).as_bytes());
}

fn vertical_edges(grid: &GridSnapshot, row: usize) -> Vec<usize> {
    grid.lines()[row]
        .chars()
        .enumerate()
        .filter_map(|(x, c)| (c == '│').then_some(x))
        .collect()
}

#[test]
fn packed_agent_mouse_resize_tracks_columns_in_a_shared_span() {
    let mut yinz = TestRunner::new(Size { cols: 400, rows: 40 })
        .with_config("pane_frame_style \"full\"\nmouse_mode true\nadvanced_mouse_actions true\nmax_agent_width 80\nauto_layout false\nload_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified("layout {\n pane\n pane split_direction=\"vertical\" { pane; pane; }\n}".into()))
        .start();
    let top = yinz.expect_pty_spawn();
    let left = yinz.expect_pty_spawn();
    let right = yinz.expect_pty_spawn();
    for (pane, text) in [(&top, "TOP"), (&left, "LEFT"), (&right, "RIGHT")] {
        pane.output(text.as_bytes());
        pane.run_foreground(&["codex"]);
    }
    let wait_for_width = |cols: usize| {
        top.wait_for_size("top PTY stays capped", |cols, rows| {
            cols == 80 && rows == 18
        });
        left.wait_for_size("packed left PTY follows mouse columns", |width, rows| {
            usize::from(width) == cols && rows == 18
        });
        right.wait_for_size("packed right PTY fills the row", |width, rows| {
            usize::from(width) == 78 - cols && rows == 18
        });
        yinz.wait_until("packed borders remain contiguous", |grid| {
            grid.contains("TOP")
                && grid.contains("LEFT")
                && grid.contains("RIGHT")
                && vertical_edges(grid, 5) == [159, 240]
                && vertical_edges(grid, 25) == [159, 160 + cols, 161 + cols, 240]
        });
    };
    wait_for_width(39);
    let mut x: usize = 199;
    let mut cols: usize = 39;
    ctrl_mouse(&yinz, x, 25, 16, false);
    for delta in [-1, -4, 2, 3, 1] {
        x = x.checked_add_signed(delta).unwrap();
        cols = cols.checked_add_signed(delta).unwrap();
        ctrl_mouse(&yinz, x, 25, 48, false);
        wait_for_width(cols);
    }
    ctrl_mouse(&yinz, x, 25, 16, true);
    ctrl_mouse(&yinz, 165, 25, 0, false);
    ctrl_mouse(&yinz, 165, 25, 0, true);
    yinz.wait_until("keyboard focus is in the lower-left pane", |grid| {
        grid.cursor_is_at(col(164).row(21))
    });
    resize_in_resize_mode(&yinz, &keys::key('L'));
    wait_for_width(20);
    yinz.run_cli_action(CliAction::Resize {
        resize: Resize::Increase,
        direction: Some(Direction::Right),
        pane_id: Some(format!("terminal_{}", left.terminal_id())),
    });
    wait_for_width(40);
    yinz.quit();
}

#[test]
fn capped_agent_mouse_and_directional_resizing_updates_borders_and_pty() {
    for limit in [0, 80] {
        for other_is_agent in [false, true] {
            let mut yinz = TestRunner::new(Size { cols: 400, rows: 20 })
                .with_config(&format!("pane_frame_style \"full\"\nmouse_mode true\nadvanced_mouse_actions true\nmax_agent_width {limit}\nauto_layout false\nload_plugins clear-defaults=true {{}}"))
                .with_layout(LayoutInfo::Stringified("layout {\n pane split_direction=\"vertical\" { pane; pane; }\n}".into()))
                .start();
            let left = yinz.expect_pty_spawn();
            let right = yinz.expect_pty_spawn();
            left.output(b"LEFT");
            right.output(b"RIGHT");
            left.run_foreground(&["codex"]);
            if other_is_agent {
                right.run_foreground(&["codex"]);
            }
            let initial = if limit == 80 { 80 } else { 198 };
            let wait_for_width = |cols: usize| {
                left.wait_for_size("resized left PTY", |width, rows| {
                    usize::from(width) == cols && rows == 18
                });
                let right_cols = if other_is_agent && limit == 80 {
                    80
                } else {
                    396 - cols
                };
                right.wait_for_size("neighbor PTY", |width, rows| {
                    usize::from(width) == right_cols && rows == 18
                });
                let start = (400 - (cols + right_cols + 4)) / 2;
                yinz.wait_until("resized borders are contiguous", |grid| {
                    grid.contains("LEFT")
                        && grid.contains("RIGHT")
                        && vertical_edges(grid, 5)
                            == [
                                start,
                                start + cols + 1,
                                start + cols + 2,
                                start + cols + right_cols + 3,
                            ]
                });
                start + cols + 1
            };
            let mut x = wait_for_width(initial);
            ctrl_mouse(&yinz, x, 5, 16, false);
            let mut cols = initial;
            for delta in [-1, -4, 2, 3] {
                x = x.checked_add_signed(delta).unwrap();
                cols = cols.checked_add_signed(delta).unwrap();
                ctrl_mouse(&yinz, x, 5, 48, false);
                wait_for_width(cols);
            }
            ctrl_mouse(&yinz, x, 5, 16, true);
            yinz.run_cli_action(CliAction::FocusPaneId {
                pane_id: format!("terminal_{}", left.terminal_id()),
            });
            resize_in_resize_mode(&yinz, &keys::key('L'));
            wait_for_width(initial - 20);
            yinz.run_cli_action(CliAction::Resize {
                resize: Resize::Increase,
                direction: Some(Direction::Right),
                pane_id: Some(format!("terminal_{}", left.terminal_id())),
            });
            wait_for_width(initial);
            yinz.quit();
        }
    }
}

#[test]
fn capped_agent_diagonal_mouse_drag_resizes_height_past_blocked_width() {
    for below in [false, true] {
        for other_is_agent in [false, true] {
            let mut yinz = TestRunner::new(Size { cols: 400, rows: 40 })
                .with_config("pane_frame_style \"full\"\nmouse_mode true\nadvanced_mouse_actions true\nmax_agent_width 80\nauto_layout false\nload_plugins clear-defaults=true {}")
                .with_layout(LayoutInfo::Stringified("layout { pane; pane; }".into()))
                .start();
            let top = yinz.expect_pty_spawn();
            let bottom = yinz.expect_pty_spawn();
            top.output(b"TOP");
            bottom.output(b"BOTTOM");
            let agent = if below { &bottom } else { &top };
            agent.run_foreground(&["codex"]);
            if other_is_agent {
                (if below { &top } else { &bottom }).run_foreground(&["codex"]);
            }
            let wait_for_boundary = |boundary: usize| {
                top.wait_for_size("top PTY follows horizontal border", |cols, rows| {
                    cols == 80 && usize::from(rows) == boundary - 2
                });
                bottom.wait_for_size("bottom PTY follows horizontal border", |cols, rows| {
                    cols == 80 && usize::from(rows) == 38 - boundary
                });
                yinz.wait_until(
                    "horizontal borders moved and widths stayed capped",
                    |grid| {
                        let lines = grid.lines();
                        grid.contains("TOP")
                            && grid.contains("BOTTOM")
                            && lines[boundary - 1].chars().nth(159) == Some('└')
                            && lines[boundary].chars().nth(159) == Some('┌')
                            && vertical_edges(grid, 5) == [159, 240]
                    },
                );
            };
            wait_for_boundary(20);
            let mut y = if below { 20 } else { 19 };
            let mut x = 240;
            ctrl_mouse(&yinz, x, y, 16, false);
            for delta in [-2, 4, -2] {
                x += 1;
                y = y.checked_add_signed(delta).unwrap();
                ctrl_mouse(&yinz, x, y, 48, false);
                wait_for_boundary(if below { y } else { y + 1 });
            }
            ctrl_mouse(&yinz, x, y, 16, true);
            yinz.quit();
        }
    }
}

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

use crate::{col, keys, FakePtyHandle, GridSnapshot, Size, TestRunner, TestSession};

pub const TERMINAL_SIZE: Size = Size {
    cols: 120,
    rows: 24,
};
pub const PROMPT: &[u8] = b"$ ";

/// Whether the tab bar is showing no lone pane's id - the settled state whenever the tab does
/// not have exactly one pane wearing a title.
///
/// A tab down to one titled pane wears that pane's id (`Tab::lone_pane_id_marker`), so the bar
/// reads "[0] Tab #1" until whatever changed that - a split, a frame style with no titles -
/// reaches it. The bar is a plugin and renders a frame or two behind the grid, so a test that
/// only waits for the grid can snapshot the bar still wearing the marker, a race that looks
/// like a flake in whichever test hits it.
pub fn tab_bar_shows_no_lone_pane_id(grid_snapshot: &GridSnapshot) -> bool {
    let Some(tab_bar) = grid_snapshot.tab_bar_line().map(str::to_owned) else {
        return false;
    };
    // A "[" with a digit behind it, which on the bar's own line is the marker and nothing else.
    let line: Vec<char> = tab_bar.chars().collect();
    !line
        .windows(2)
        .any(|window| window[0] == '[' && window[1].is_ascii_digit())
}

pub fn start_yinz() -> TestSession {
    TestRunner::new(TERMINAL_SIZE).start()
}

pub fn claim_first_terminal_and_wait_for_prompt(yinz: &TestSession) -> FakePtyHandle {
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until(
        "first terminal prompt rendered in loaded app",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(2).row(1))
        },
    );
    terminal
}

pub fn split_right_and_wait_for_prompt(yinz: &TestSession) -> FakePtyHandle {
    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('r'));
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until("right terminal prompt rendered", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.cursor_is_at(col(62).row(2))
            && tab_bar_shows_no_lone_pane_id(grid_snapshot)
    });
    terminal
}

pub fn split_down_and_wait_for_prompt(yinz: &TestSession) -> FakePtyHandle {
    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('d'));
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until("lower terminal prompt rendered", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.cursor_is_at(col(2).row(13))
            && tab_bar_shows_no_lone_pane_id(grid_snapshot)
    });
    terminal
}

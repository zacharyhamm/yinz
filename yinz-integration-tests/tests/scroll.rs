#![cfg(unix)]

use insta::assert_snapshot;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, normalized, start_yinz, FakePtyHandle,
    TestSession,
};

const LAST_LINE: &str = "line40";

fn fill_pane_past_viewport(yinz: &TestSession) -> FakePtyHandle {
    let terminal = claim_first_terminal_and_wait_for_prompt(yinz);
    for line in 1..=40 {
        terminal.output(format!("line{:02}\r\n", line).as_bytes());
    }
    yinz.wait_until("pane filled past its viewport", |grid_snapshot| {
        grid_snapshot.contains(LAST_LINE)
    });
    terminal
}

#[test]
fn page_scroll_up() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::ctrl('b'));

    let grid_snapshot = yinz.wait_until("scrolled up by a page", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp")
            && !grid_snapshot.contains(LAST_LINE)
            && grid_snapshot.contains("SCROLL 19/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn page_scroll_down() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until("scrolled up by a page", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp") && !grid_snapshot.contains(LAST_LINE)
    });

    yinz.send_stdin(&keys::ctrl('f'));

    // Landing back at the bottom leaves Scroll mode: the mode follows the pane's scroll
    // position, so the status bar here is the normal-mode one and not the scroll hints.
    let grid_snapshot = yinz.wait_until("scrolled back down toward the bottom", |grid_snapshot| {
        grid_snapshot.contains("line39")
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("SCROLL 0/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn half_page_scroll_up() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::key('u'));

    let grid_snapshot = yinz.wait_until("scrolled up by half a page", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp")
            && !grid_snapshot.contains(LAST_LINE)
            && grid_snapshot.contains("SCROLL 10/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn half_page_scroll_down() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::key('u'));
    yinz.wait_until("scrolled up by half a page", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp") && !grid_snapshot.contains(LAST_LINE)
    });

    yinz.send_stdin(&keys::key('d'));

    // As in `page_scroll_down`: back at the bottom is back in the default mode.
    let grid_snapshot = yinz.wait_until("scrolled back down toward the bottom", |grid_snapshot| {
        grid_snapshot.contains("line39")
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("SCROLL 0/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn scroll_down_one_line() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::key('k'));
    yinz.send_stdin(&keys::key('k'));
    yinz.wait_until("scrolled up two lines", |grid_snapshot| {
        grid_snapshot.contains("SCROLL 2/")
    });

    yinz.send_stdin(&keys::key('j'));

    let grid_snapshot = yinz.wait_until("scrolled back down one line", |grid_snapshot| {
        grid_snapshot.contains("SCROLL 1/") && grid_snapshot.contains("PgDn|PgUp")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn scroll_to_bottom() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until("scrolled up by a page", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp") && !grid_snapshot.contains(LAST_LINE)
    });

    yinz.send_stdin(&keys::ctrl('c'));

    let grid_snapshot = yinz.wait_until("returned to the bottom in normal mode", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains(LAST_LINE)
            && grid_snapshot.contains("SCROLL 0/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

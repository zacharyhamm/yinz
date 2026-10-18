#![cfg(unix)]

use insta::assert_snapshot;
use yinz_integration_tests::{
    assert_same_rendered_grid, claim_first_terminal_and_wait_for_prompt, col, keys, normalized,
    split_down_and_wait_for_prompt, split_right_and_wait_for_prompt, start_yinz,
    tab_bar_shows_no_lone_pane_id, Coord, Size, TestRunner, TestSession, PROMPT, TERMINAL_SIZE,
};
use yinz_types::data::LayoutInfo;

#[test]
fn cannot_split_terminals_vertically_when_active_terminal_is_too_small() {
    let mut yinz = TestRunner::new(Size { cols: 8, rows: 20 }).start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until("first terminal prompt rendered", |grid_snapshot| {
        grid_snapshot.cursor_is_at(col(2).row(2))
    });
    let (width_before_split, _) = terminal.wait_for_size("first terminal sized", |_, _| true);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('r'));

    terminal.output(b"done");
    let tab_bar_glyph = '\u{e0b0}';
    let grid_snapshot = yinz.wait_until(
        "split attempt processed, chrome rendered",
        |grid_snapshot| {
            grid_snapshot.contains("done")
                && grid_snapshot.contains("Ctrl +")
                && grid_snapshot
                    .lines()
                    .get(1)
                    .is_some_and(|first_line| first_line.contains(tab_bar_glyph))
        },
    );

    assert_eq!(terminal.size().unwrap().0, width_before_split);
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn key_after_mode_switch_is_interpreted_in_new_mode_not_leaked_to_pane() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('r'));

    let second_terminal = yinz.expect_pty_spawn();
    second_terminal.output(PROMPT);
    yinz.wait_until("split-right interpreted in pane mode", |grid_snapshot| {
        grid_snapshot.cursor_is_at(col(62).row(3))
    });

    assert!(!first_terminal.stdin_bytes().contains(&keys::key('r')[0]));
    yinz.quit();
}

#[test]
fn toggle_pane_fullscreen() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('f'));

    let grid_snapshot = yinz.wait_until("focused pane is fullscreen", |grid_snapshot| {
        grid_snapshot.cursor_is_at(col(2).row(3))
            && grid_snapshot.contains("LOCK")
            && grid_snapshot.contains("(FULLSCREEN)")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn new_pane_while_fullscreen_breaks_out_of_fullscreen() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('f'));
    yinz.wait_until("focused pane is fullscreen", |grid_snapshot| {
        grid_snapshot.contains("(FULLSCREEN)")
    });

    yinz.send_stdin(&keys::alt('n'));
    let new_terminal = yinz.expect_pty_spawn();
    new_terminal.output(PROMPT);

    let grid_snapshot = yinz.wait_until(
        "all three panes tiled after breaking out of fullscreen",
        |grid_snapshot| {
            !grid_snapshot.contains("(FULLSCREEN)")
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("Pane #1")
                && grid_snapshot.contains("Pane #2")
                && grid_snapshot.contains("Pane #3")
                && grid_snapshot.cursor.is_some()
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn closing_the_fullscreen_pane_restores_the_remaining_layout() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);
    let two_pane_state = yinz.wait_until("two panes settled", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(62).row(3))
    });

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('d'));
    let third_terminal = yinz.expect_pty_spawn();
    third_terminal.output(PROMPT);
    yinz.wait_until("third pane split below the right pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(62).row(14))
    });

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('f'));
    yinz.wait_until("third pane is fullscreen", |grid_snapshot| {
        grid_snapshot.contains("(FULLSCREEN)")
    });

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('x'));
    let after_close = yinz.wait_until(
        "fullscreen pane closed, two panes restored",
        |grid_snapshot| {
            !grid_snapshot.contains("(FULLSCREEN)")
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(62).row(3))
        },
    );

    assert_same_rendered_grid(
        &after_close,
        &two_pane_state,
        "closing the fullscreen pane restores the pre-split layout",
    );
    yinz.quit();
}

#[test]
fn close_pane() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('x'));

    let grid_snapshot =
        yinz.wait_until("right pane closed, focus back on first", |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(2).row(2))
                && grid_snapshot
                    .tab_bar_line()
                    .is_some_and(|bar| bar.contains("[0] Tab #1"))
        });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn closing_last_pane_exits_when_no_background_panes_remain() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('x'));

    yinz.wait_until("yinz exited after closing the last pane", |grid_snapshot| {
        grid_snapshot.contains("Bye from Yinz!")
    });
    yinz.wait_for_exit();
}

#[test]
fn last_terminal_exit_ends_session_with_background_plugins() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    terminal.exit(Some(0));

    yinz.wait_until("yinz exited after the last shell exited", |grid_snapshot| {
        grid_snapshot.contains("Bye from Yinz!")
    });
    yinz.wait_for_exit();
}

#[test]
fn last_terminal_exit_ends_session_without_plugin_renders() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".to_owned()))
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until("shell prompt rendered", |grid_snapshot| {
        grid_snapshot.contains("$ ")
    });

    terminal.exit(Some(0));

    yinz.wait_until("yinz exited after the last shell exited", |grid_snapshot| {
        grid_snapshot.contains("Bye from Yinz!")
    });
    yinz.wait_for_exit();
}

#[test]
fn pane_closes_when_its_process_exits() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);

    right_terminal.exit(Some(0));

    let grid_snapshot =
        yinz.wait_until("right pane closed, focus back on first", |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(2).row(2))
                && grid_snapshot
                    .tab_bar_line()
                    .is_some_and(|bar| bar.contains("[0] Tab #1"))
        });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn last_tiled_exit_reveals_hidden_floating_pane_and_delivers_keyboard_input() {
    use yinz_types::input::actions::Action;
    use yinz_types::internal_ids::TabNumber;

    for another_tab in [false, true] {
        let mut yinz = start_yinz();
        let tiled = claim_first_terminal_and_wait_for_prompt(&yinz);
        tiled.output(b"EXITING-TILED");
        let other = if another_tab {
            yinz.send_stdin(&keys::ctrl('t'));
            yinz.send_stdin(&keys::key('n'));
            let other = yinz.expect_pty_spawn();
            other.output(b"OTHER-TAB");
            yinz.wait_until("second tab focused", |grid| grid.contains("OTHER-TAB"));
            assert_eq!(
                yinz.run_actions_reporting(vec![Action::GoToTab {
                    index: TabNumber::new(1)
                }])
                .0,
                0
            );
            Some(other)
        } else {
            None
        };
        yinz.wait_until("source tab focused", |grid| grid.contains("EXITING-TILED"));
        assert_eq!(
            yinz.run_actions_reporting(vec![Action::ToggleFloatingPanes])
                .0,
            0
        );
        let floating = yinz.expect_pty_spawn();
        floating.output(b"SURVIVING-FLOAT");
        yinz.wait_until("floating pane visible", |grid| {
            grid.contains("SURVIVING-FLOAT")
        });
        assert_eq!(
            yinz.run_actions_reporting(vec![Action::ToggleFloatingPanes])
                .0,
            0
        );
        yinz.wait_until("floating pane hidden", |grid| {
            grid.contains("EXITING-TILED") && !grid.contains("SURVIVING-FLOAT")
        });

        tiled.exit(Some(0));
        yinz.wait_until("surviving floating pane revealed and focused", |grid| {
            grid.contains("SURVIVING-FLOAT")
                && !grid.contains("EXITING-TILED")
                && grid.cursor.is_some()
        });
        yinz.send_stdin(b"focus-survived");
        floating.wait_for_stdin("keyboard reaches the surviving pane", |bytes| {
            bytes
                .windows(b"focus-survived".len())
                .any(|window| window == b"focus-survived")
        });
        if let Some(other) = other {
            assert!(!other
                .stdin_bytes()
                .windows(b"focus-survived".len())
                .any(|window| window == b"focus-survived"));
        }
        yinz.quit();
    }
}

#[test]
fn toggle_floating_panes() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('w'));
    let floating_terminal = yinz.expect_pty_spawn();
    floating_terminal.output(PROMPT);

    let grid_snapshot = yinz.wait_until("floating pane appeared", |grid_snapshot| {
        grid_snapshot.cursor_is_at(col(33).row(8))
            && grid_snapshot.contains("STAGGERED")
            && grid_snapshot.contains("LOCK")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn undo_rename_pane() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('c'));
    yinz.send_stdin(b"aa");

    yinz.wait_until("typed name applied in rename mode", |grid_snapshot| {
        grid_snapshot.contains("aa") && grid_snapshot.contains("RENAMING PANE")
    });

    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("rename undone, back in pane mode", |grid_snapshot| {
        grid_snapshot.contains("Pane #2") && !grid_snapshot.contains("aa")
    });

    yinz.send_stdin(&keys::ESC);
    let grid_snapshot = yinz.wait_until("pane name reverted to default", |grid_snapshot| {
        grid_snapshot.contains("Pane #2")
            && !grid_snapshot.contains("aa")
            && grid_snapshot.contains("LOCK")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

/// The stacked spelling of [`spawn_second_pane_and_wait_for_grid`].
///
/// A stack is one content pane, so the tab keeps wearing a pane id - that of the member on
/// screen, here the new one. Waiting for the bar to name it is what keeps a snapshot off the
/// frame where the bar still names the member the stack was showing before.
fn spawn_second_stacked_pane_and_wait_for_grid(
    yinz: &TestSession,
    prompt: Coord,
) -> yinz_integration_tests::GridSnapshot {
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until(
        "second stacked pane spawned and prompt rendered",
        move |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("Pane #2")
                && grid_snapshot.cursor_is_at(prompt)
                && grid_snapshot
                    .tab_bar_line()
                    .is_some_and(|tab_bar| tab_bar.contains("[1] "))
        },
    )
}

fn spawn_second_pane_and_wait_for_grid(
    yinz: &TestSession,
    prompt: Coord,
) -> yinz_integration_tests::GridSnapshot {
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until(
        "second pane spawned and prompt rendered",
        move |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("Pane #2")
                && grid_snapshot.cursor_is_at(prompt)
                && tab_bar_shows_no_lone_pane_id(grid_snapshot)
        },
    )
}

fn appears_in_order(grid_snapshot: &yinz_integration_tests::GridSnapshot, labels: &[&str]) -> bool {
    let mut search_from = 0;
    for label in labels {
        match grid_snapshot.text[search_from..].find(label) {
            Some(offset) => search_from += offset + label.len(),
            None => return false,
        }
    }
    true
}

#[test]
fn move_focus_up_in_pane_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_down_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('k'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("focus moved to the upper pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(2).row(3))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_focus_down_in_pane_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_down_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('k'));
    yinz.wait_until("focus moved to the upper pane", |grid_snapshot| {
        grid_snapshot.cursor_is_at(col(2).row(3))
    });

    yinz.send_stdin(&keys::key('j'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("focus moved back to the lower pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(2).row(14))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_pane_swaps_with_neighbor() {
    let mut yinz = start_yinz();
    let left_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);
    left_terminal.output(b"AAA");
    right_terminal.output(b"BBB");
    yinz.wait_until("both panes labelled left to right", |grid_snapshot| {
        appears_in_order(grid_snapshot, &["AAA", "BBB"])
    });

    yinz.send_stdin(&keys::ctrl('h'));
    yinz.send_stdin(&keys::key('n'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("focused pane swapped to the left", |grid_snapshot| {
        appears_in_order(grid_snapshot, &["BBB", "AAA"]) && grid_snapshot.status_bar_appears()
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_pane_backwards_swaps_with_neighbor() {
    let mut yinz = start_yinz();
    let left_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);
    left_terminal.output(b"AAA");
    right_terminal.output(b"BBB");
    yinz.wait_until("both panes labelled left to right", |grid_snapshot| {
        appears_in_order(grid_snapshot, &["AAA", "BBB"])
    });

    yinz.send_stdin(&keys::ctrl('h'));
    yinz.send_stdin(&keys::key('p'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("focused pane swapped backwards", |grid_snapshot| {
        appears_in_order(grid_snapshot, &["BBB", "AAA"]) && grid_snapshot.status_bar_appears()
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_pane_left() {
    let mut yinz = start_yinz();
    let left_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);
    left_terminal.output(b"AAA");
    right_terminal.output(b"BBB");
    yinz.wait_until("both panes labelled left to right", |grid_snapshot| {
        appears_in_order(grid_snapshot, &["AAA", "BBB"])
    });

    yinz.send_stdin(&keys::ctrl('h'));
    yinz.send_stdin(&keys::key('h'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("focused pane moved to the left", |grid_snapshot| {
        appears_in_order(grid_snapshot, &["BBB", "AAA"]) && grid_snapshot.status_bar_appears()
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_pane_down() {
    let mut yinz = start_yinz();
    let upper_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    let lower_terminal = split_down_and_wait_for_prompt(&yinz);
    upper_terminal.output(b"AAA");
    lower_terminal.output(b"BBB");
    yinz.wait_until("both panes labelled top to bottom", |grid_snapshot| {
        appears_in_order(grid_snapshot, &["AAA", "BBB"])
    });

    yinz.send_stdin(&keys::ctrl('h'));
    yinz.send_stdin(&keys::key('k'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("focused pane moved upward", |grid_snapshot| {
        appears_in_order(grid_snapshot, &["BBB", "AAA"]) && grid_snapshot.status_bar_appears()
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn toggle_pane_embed_or_floating() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('e'));

    let grid_snapshot = yinz.wait_until("focused pane became floating", |grid_snapshot| {
        grid_snapshot.contains("STAGGERED")
            && grid_snapshot.contains("LOCK")
            && grid_snapshot.cursor_is_at(col(33).row(8))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn toggle_pane_pinned() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('w'));
    let floating_terminal = yinz.expect_pty_spawn();
    floating_terminal.output(PROMPT);
    yinz.wait_until("floating pane appeared", |grid_snapshot| {
        grid_snapshot.contains("STAGGERED")
    });

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('i'));

    let grid_snapshot = yinz.wait_until("floating pane pinned", |grid_snapshot| {
        grid_snapshot.contains("PIN [+]")
            && grid_snapshot.contains("LOCK")
            && grid_snapshot.cursor_is_at(col(33).row(8))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_focus_or_tab_moves_focus_between_panes() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::alt('h'));

    let grid_snapshot = yinz.wait_until("focus moved to the left pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(2).row(3))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_focus_or_tab_switches_tab_when_single_pane() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    first_terminal.output(b"oneone");
    yinz.wait_until("first tab pane labelled", |grid_snapshot| {
        grid_snapshot.contains("oneone")
    });

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('n'));
    let second_terminal = yinz.expect_pty_spawn();
    second_terminal.output(PROMPT);
    yinz.wait_until("second tab focused, first tab hidden", |grid_snapshot| {
        grid_snapshot.contains("Tab #2") && !grid_snapshot.contains("oneone")
    });

    yinz.send_stdin(&keys::alt('h'));

    // `[1] Tab #2` has to be back on the bar before this settles: the first tab's own tab-bar
    // instance renders its cached state, from when it was last active and the second tab did
    // not exist, before it catches up. Without this the snapshot can land on that stale frame.
    let grid_snapshot = yinz.wait_until("focus switched back to the first tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("oneone")
            && grid_snapshot.contains("[1] Tab #2")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn new_pane_in_pane_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('n'));
    let grid_snapshot = spawn_second_pane_and_wait_for_grid(&yinz, col(62).row(3));
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn new_pane_in_normal_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::alt('n'));
    let grid_snapshot = spawn_second_pane_and_wait_for_grid(&yinz, col(62).row(3));
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn split_pane_downward() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('d'));
    let grid_snapshot = spawn_second_pane_and_wait_for_grid(&yinz, col(2).row(14));
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn new_stacked_pane() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('s'));
    let grid_snapshot = spawn_second_stacked_pane_and_wait_for_grid(&yinz, col(2).row(5));
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_focus_left_in_pane_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('h'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("focus moved to the left pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(2).row(3))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_focus_right_in_pane_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('h'));
    yinz.wait_until("focus moved to the left pane", |grid_snapshot| {
        grid_snapshot.cursor_is_at(col(2).row(3))
    });

    yinz.send_stdin(&keys::key('l'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("focus moved back to the right pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(62).row(3))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn switch_focus_in_pane_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('p'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("focus switched to the other pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(2).row(3))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

fn cycle_pane_frames(yinz: &TestSession) {
    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('z'));
}

#[test]
fn toggle_pane_frames() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.wait_until("tiled panes start in titles mode", |grid_snapshot| {
        grid_snapshot.contains("Pane #1")
            && grid_snapshot.contains("Pane #2")
            && !grid_snapshot.contains("┌")
    });

    cycle_pane_frames(&yinz);
    yinz.wait_until("titles cycle to frameless", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && !grid_snapshot.contains("Pane #1")
            && !grid_snapshot.contains("Pane #2")
            && !grid_snapshot.contains("┌")
            && grid_snapshot.cursor_is_at(col(62).row(2))
    });

    cycle_pane_frames(&yinz);
    yinz.wait_until("frameless cycles to full frames", |grid_snapshot| {
        grid_snapshot.contains("┌")
            && grid_snapshot.contains("Pane #1")
            && grid_snapshot.contains("Pane #2")
    });

    cycle_pane_frames(&yinz);
    let grid_snapshot = yinz.wait_until("full frames cycle back to titles", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("Pane #1")
            && grid_snapshot.contains("Pane #2")
            && !grid_snapshot.contains("┌")
            && tab_bar_shows_no_lone_pane_id(grid_snapshot)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn toggle_frames_with_single_pane() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.wait_until(
        "lone pane in titles mode omits its title",
        |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && !grid_snapshot.contains("Pane #1")
                && !grid_snapshot.contains("┌")
        },
    );

    cycle_pane_frames(&yinz);
    yinz.wait_until("lone frameless pane stays untitled", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && !grid_snapshot.contains("Pane #1")
            && !grid_snapshot.contains("┌")
    });

    cycle_pane_frames(&yinz);
    let grid_snapshot =
        yinz.wait_until("lone pane gains a full frame and title", |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("┌")
                && grid_snapshot.contains("Pane #1")
                // A full frame carries the id itself, so the bar drops it.
                && tab_bar_shows_no_lone_pane_id(grid_snapshot)
        });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn toggle_frames_with_floating_pane() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('w'));
    let floating_terminal = yinz.expect_pty_spawn();
    floating_terminal.output(PROMPT);
    yinz.wait_until("floating pane appeared with a frame", |grid_snapshot| {
        grid_snapshot.contains("STAGGERED") && grid_snapshot.contains("┌")
    });

    cycle_pane_frames(&yinz);
    yinz.wait_until(
        "floating pane keeps its frame in frameless mode",
        |grid_snapshot| grid_snapshot.status_bar_appears() && grid_snapshot.contains("┌"),
    );

    cycle_pane_frames(&yinz);
    let grid_snapshot = yinz.wait_until(
        "floating pane keeps its frame in full mode",
        |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("┌")
                && grid_snapshot.contains("Pane #1")
                // A full frame carries the id itself, so the bar drops it.
                && tab_bar_shows_no_lone_pane_id(grid_snapshot)
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn toggle_frames_with_stacked_panes() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('s'));
    spawn_second_stacked_pane_and_wait_for_grid(&yinz, col(2).row(5));

    yinz.wait_until("stacked panes show their titles", |grid_snapshot| {
        grid_snapshot.contains("Pane #1")
            && grid_snapshot.contains("Pane #2")
            && !grid_snapshot.contains("┌")
    });

    cycle_pane_frames(&yinz);
    yinz.wait_until("stacked panes cycle to frameless", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && !grid_snapshot.contains("┌")
    });

    cycle_pane_frames(&yinz);
    let grid_snapshot = yinz.wait_until("stacked panes cycle to full frames", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("┌")
            && grid_snapshot.contains("Pane #1")
            && grid_snapshot.contains("Pane #2")
            && tab_bar_shows_no_lone_pane_id(grid_snapshot)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn start_without_pane_frames() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("pane_frames false")
        .start();
    let first_terminal = yinz.expect_pty_spawn();
    first_terminal.output(PROMPT);
    yinz.wait_until(
        "first frameless terminal prompt rendered",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(2).row(2))
        },
    );

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('r'));
    let right_terminal = yinz.expect_pty_spawn();
    right_terminal.output(PROMPT);

    let grid_snapshot = yinz.wait_until(
        "right frameless terminal prompt rendered",
        |grid_snapshot| {
            grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(62).row(2))
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

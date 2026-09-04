use super::*;

#[test]
fn selection_start() {
    let mut selection = Selection::default();
    selection.start(Position::new(10, 10));

    assert!(selection.active);
    assert_eq!(selection.start, Position::new(10, 10));
    assert_eq!(selection.end, Position::new(10, 10));
}

#[test]
fn selection_to() {
    let mut selection = Selection::default();
    selection.start(Position::new(10, 10));
    let is_active = selection.active;
    selection.to(Position::new(20, 30));

    assert_eq!(selection.active, is_active);
    assert_eq!(selection.end, Position::new(20, 30));
}

#[test]
fn selection_end() {
    let mut selection = Selection::default();
    selection.start(Position::new(10, 10));
    selection.end(Position::new(20, 30));

    assert!(!selection.active);
    assert_eq!(selection.end, Position::new(20, 30));
}

#[test]
fn swap_ends_round_trips_and_keeps_the_selected_region() {
    let mut selection = Selection::default();
    selection.start(Position::new(10, 5));
    selection.to(Position::new(10, 9));
    let contained: Vec<usize> = (0..12).filter(|col| selection.contains(10, *col)).collect();

    selection.swap_ends();
    assert_eq!(selection.start, Position::new(10, 9));
    assert_eq!(selection.end, Position::new(10, 5));
    assert!(selection.active, "the selection is still being moved");
    assert_eq!(
        (0..12)
            .filter(|col| selection.contains(10, *col))
            .collect::<Vec<usize>>(),
        contained,
        "swapping the ends does not change which cells are selected"
    );

    selection.swap_ends();
    assert_eq!(selection.start, Position::new(10, 5));
    assert_eq!(selection.end, Position::new(10, 9));
}

#[test]
fn add_word_to_position_extend_line_above() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_word_start = Position::new(10, 10);
    let last_word_end = Position::new(10, 15);
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: Some((last_word_start, last_word_end)),
        last_added_line_index: None,
    };
    let word_start = Position::new(9, 5);
    let word_end = Position::new(9, 6);
    selection.add_word_to_position(word_start, word_end);

    assert_eq!(selection.start, word_start);
    assert_eq!(selection.end, selection_end);
}

#[test]
fn add_word_to_position_extend_line_below() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_word_start = Position::new(20, 15);
    let last_word_end = Position::new(20, 20);
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: Some((last_word_start, last_word_end)),
        last_added_line_index: None,
    };
    let word_start = Position::new(21, 5);
    let word_end = Position::new(21, 6);
    selection.add_word_to_position(word_start, word_end);

    assert_eq!(selection.start, selection_start);
    assert_eq!(selection.end, word_end);
}

#[test]
fn add_word_to_position_reduce_from_above() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_word_start = Position::new(10, 10);
    let last_word_end = Position::new(10, 20);
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: Some((last_word_start, last_word_end)),
        last_added_line_index: None,
    };
    let word_start = Position::new(11, 5);
    let word_end = Position::new(11, 6);
    selection.add_word_to_position(word_start, word_end);

    assert_eq!(selection.start, word_start);
    assert_eq!(selection.end, selection_end);
}

#[test]
fn add_word_to_position_reduce_from_below() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_word_start = Position::new(20, 10);
    let last_word_end = Position::new(20, 20);
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: Some((last_word_start, last_word_end)),
        last_added_line_index: None,
    };
    let word_start = Position::new(19, 5);
    let word_end = Position::new(19, 6);
    selection.add_word_to_position(word_start, word_end);

    assert_eq!(selection.start, selection_start);
    assert_eq!(selection.end, word_end);
}

#[test]
fn add_word_to_position_extend_right() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_word_start = Position::new(20, 10);
    let last_word_end = Position::new(20, 20);
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: Some((last_word_start, last_word_end)),
        last_added_line_index: None,
    };
    let word_start = Position::new(20, 21);
    let word_end = Position::new(20, 23);
    selection.add_word_to_position(word_start, word_end);

    assert_eq!(selection.start, selection_start);
    assert_eq!(selection.end, word_end);
}

#[test]
fn add_word_to_position_extend_left() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_word_start = Position::new(10, 10);
    let last_word_end = Position::new(10, 20);
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: Some((last_word_start, last_word_end)),
        last_added_line_index: None,
    };
    let word_start = Position::new(10, 5);
    let word_end = Position::new(10, 9);
    selection.add_word_to_position(word_start, word_end);

    assert_eq!(selection.start, word_start);
    assert_eq!(selection.end, selection_end);
}

#[test]
fn add_word_to_position_reduce_from_left() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_word_start = Position::new(10, 10);
    let last_word_end = Position::new(10, 20);
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: Some((last_word_start, last_word_end)),
        last_added_line_index: None,
    };
    let word_start = Position::new(10, 20);
    let word_end = Position::new(10, 30);
    selection.add_word_to_position(word_start, word_end);

    assert_eq!(selection.start, word_start);
    assert_eq!(selection.end, selection_end);
}

#[test]
fn add_word_to_position_reduce_from_right() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_word_start = Position::new(20, 10);
    let last_word_end = Position::new(20, 20);
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: Some((last_word_start, last_word_end)),
        last_added_line_index: None,
    };
    let word_start = Position::new(20, 5);
    let word_end = Position::new(20, 10);
    selection.add_word_to_position(word_start, word_end);

    assert_eq!(selection.start, selection_start);
    assert_eq!(selection.end, word_end);
}

#[test]
fn add_line_to_position_extend_upwards() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_added_line_index = 10;
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: None,
        last_added_line_index: Some(last_added_line_index),
    };
    let line_index_to_add = 9;
    let last_index_in_line = 21;
    selection.add_line_to_position(line_index_to_add, last_index_in_line);

    assert_eq!(selection.start, Position::new(line_index_to_add as i32, 0));
    assert_eq!(selection.end, selection_end);
}

#[test]
fn add_line_to_position_extend_downwards() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_added_line_index = 20;
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: None,
        last_added_line_index: Some(last_added_line_index),
    };
    let line_index_to_add = 21;
    let last_index_in_line = 21;
    selection.add_line_to_position(line_index_to_add, last_index_in_line);

    assert_eq!(selection.start, selection_start);
    assert_eq!(
        selection.end,
        Position::new(line_index_to_add as i32, last_index_in_line as u16)
    );
}

#[test]
fn add_line_to_position_reduce_from_below() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_added_line_index = 20;
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: None,
        last_added_line_index: Some(last_added_line_index),
    };
    let line_index_to_add = 19;
    let last_index_in_line = 21;
    selection.add_line_to_position(line_index_to_add, last_index_in_line);

    assert_eq!(selection.start, selection_start);
    assert_eq!(
        selection.end,
        Position::new(line_index_to_add as i32, last_index_in_line as u16)
    );
}

#[test]
fn add_line_to_position_reduce_from_above() {
    let selection_start = Position::new(10, 10);
    let selection_end = Position::new(20, 20);
    let last_added_line_index = 10;
    let mut selection = Selection {
        start: selection_start,
        end: selection_end,
        active: true,
        last_added_word_position: None,
        last_added_line_index: Some(last_added_line_index),
    };
    let line_index_to_add = 9;
    let last_index_in_line = 21;
    selection.add_line_to_position(line_index_to_add, last_index_in_line);

    assert_eq!(selection.start, Position::new(line_index_to_add as i32, 0));
    assert_eq!(selection.end, selection_end);
}

#[test]
fn finalize_sets_active_to_false() {
    let mut selection = Selection::default();
    selection.start(Position::new(10, 10));
    assert!(selection.active);

    selection.finalize();
    assert!(!selection.active);
}

#[test]
fn finalize_preserves_positions() {
    let start = Position::new(5, 10);
    let end = Position::new(15, 20);
    let mut selection = Selection {
        start,
        end,
        active: true,
        last_added_word_position: Some((start, end)),
        last_added_line_index: None,
    };

    selection.finalize();

    assert_eq!(selection.start, start);
    assert_eq!(selection.end, end);
    assert!(!selection.active);
}

#[test]
fn double_click_then_scroll_moves_both_positions() {
    let word_start = Position::new(10, 5);
    let word_end = Position::new(10, 15);
    let mut selection = Selection::default();

    selection.set_start_and_end_positions(word_start, word_end);
    assert!(selection.active);

    selection.finalize();
    assert!(!selection.active);

    selection.move_down(3);

    assert_eq!(selection.start, Position::new(13, 5));
    assert_eq!(selection.end, Position::new(13, 15));
}

#[test]
fn triple_click_then_scroll_moves_both_positions() {
    let line_start = Position::new(10, 0);
    let line_end = Position::new(10, 80);
    let mut selection = Selection::default();

    selection.set_start_and_end_positions(line_start, line_end);
    selection.finalize();

    selection.move_up(5);

    assert_eq!(selection.start, Position::new(5, 0));
    assert_eq!(selection.end, Position::new(5, 80));
}

#[test]
fn set_start_and_end_without_finalize_only_moves_start() {
    let word_start = Position::new(10, 5);
    let word_end = Position::new(10, 15);
    let mut selection = Selection::default();

    selection.set_start_and_end_positions(word_start, word_end);
    assert!(selection.active);

    selection.move_down(3);

    assert_eq!(selection.start, Position::new(13, 5));
    assert_eq!(selection.end, word_end);
}

mod props {
    use super::*;
    use hegel::generators::{self};

    #[hegel::composite]
    fn positions(tc: &hegel::TestCase) -> Position {
        // Lines are `isize` internally but `Position::new` takes i32/u16, and
        // move_up/move_down shift lines without saturating — stay well inside
        // the range so the *test* can't be what overflows.
        let line = tc.draw(
            generators::integers::<i32>()
                .min_value(-100_000)
                .max_value(100_000),
        );
        let column = tc.draw(generators::integers::<u16>());
        Position::new(line, column)
    }

    #[hegel::composite]
    fn selections(tc: &hegel::TestCase) -> Selection {
        let start = tc.draw(positions());
        let end = tc.draw(positions());
        let active = tc.draw(generators::booleans());
        Selection {
            start,
            end,
            active,
            last_added_word_position: None,
            last_added_line_index: None,
        }
    }

    /// Independent oracle for `contains`: a selection covers a contiguous run
    /// of cells in reading order, from `start` inclusive to `end` exclusive.
    /// The six hand-picked `TestCase` rows this replaced were points on it.
    fn oracle_contains(selection: &Selection, row: usize, col: usize) -> bool {
        let sorted = selection.sorted();
        let cell = (row as isize, col);
        let start = (sorted.start.line.0, sorted.start.column.0);
        let end = (sorted.end.line.0, sorted.end.column.0);
        cell >= start && cell < end
    }

    #[hegel::test]
    fn contains_agrees_with_a_reading_order_range(tc: hegel::TestCase) {
        let selection = tc.draw(selections());
        // Query near the selection's own lines, otherwise almost every probe
        // lands outside and the test proves nothing.
        let sorted = selection.sorted();
        let low = sorted.start.line.0.max(0);
        let high = sorted.end.line.0.max(0);
        let row = tc.draw(
            generators::integers::<isize>()
                .min_value((low - 2).max(0))
                .max_value(high + 2),
        ) as usize;
        let col = tc.draw(generators::integers::<usize>().max_value(u16::MAX as usize));
        assert_eq!(
            selection.contains(row, col),
            oracle_contains(&selection, row, col),
            "contains({}, {}) disagreed with the oracle for {:?}",
            row,
            col,
            selection
        );
    }

    #[hegel::test]
    fn sorted_orders_the_endpoints_and_is_idempotent(tc: hegel::TestCase) {
        let selection = tc.draw(selections());
        let once = selection.sorted();
        assert!(
            once.start <= once.end,
            "{:?} sorted to {:?} which is still inverted",
            selection,
            once
        );
        assert_eq!(once, once.sorted(), "sorted() is not idempotent");
    }

    #[hegel::test]
    fn line_indices_covers_exactly_the_rows_contains_row_accepts(tc: hegel::TestCase) {
        let selection = tc.draw(selections());
        let indices = selection.line_indices();
        // `contains_row` takes a usize, so only non-negative rows are
        // reachable; a wholly-negative selection leaves nothing to probe.
        let lo = (*indices.start() - 2).max(0);
        let hi = (indices.end() + 2).max(lo);
        let row = tc.draw(generators::integers::<isize>().min_value(lo).max_value(hi));
        assert_eq!(
            selection.contains_row(row as usize),
            indices.contains(&row),
            "contains_row({}) disagreed with line_indices() {:?} for {:?}",
            row,
            selection.line_indices(),
            selection
        );
    }

    #[hegel::test]
    fn a_contained_cell_is_always_on_a_contained_row(tc: hegel::TestCase) {
        let selection = tc.draw(selections());
        let sorted = selection.sorted();
        let row = tc.draw(
            generators::integers::<isize>()
                .min_value(sorted.start.line.0.max(0))
                .max_value(sorted.end.line.0.max(0)),
        ) as usize;
        let col = tc.draw(generators::integers::<usize>().max_value(u16::MAX as usize));
        if selection.contains(row, col) {
            assert!(
                selection.contains_row(row),
                "{:?} contains cell ({}, {}) but not row {}",
                selection,
                row,
                col,
                row
            );
        }
    }

    /// Replaces move_up_inactive / move_up_active / move_down_inactive /
    /// move_down_active: scrolling up then back down restores the selection,
    /// and an active selection anchors its end either way.
    #[hegel::test]
    fn scrolling_up_then_down_by_the_same_amount_restores_the_selection(tc: hegel::TestCase) {
        let original = tc.draw(selections());
        let lines = tc.draw(generators::integers::<usize>().max_value(100_000));
        let mut selection = original;
        selection.move_up(lines);
        selection.move_down(lines);
        assert_eq!(
            (selection.start, selection.end),
            (original.start, original.end),
            "move_up({0}) then move_down({0}) did not restore {1:?}",
            lines,
            original
        );
    }

    #[hegel::test]
    fn an_active_selection_anchors_its_end_while_scrolling(tc: hegel::TestCase) {
        let original = tc.draw(selections());
        let lines = tc.draw(generators::integers::<usize>().max_value(100_000));
        let mut selection = original;
        selection.move_up(lines);
        if original.active {
            assert_eq!(
                selection.end, original.end,
                "active selection {:?} moved its end",
                original
            );
        } else {
            assert_eq!(
                selection.end.line.0,
                original.end.line.0 - lines as isize,
                "inactive selection {:?} did not move its end",
                original
            );
        }
    }

    #[hegel::test]
    fn offset_shifts_every_contained_cell_by_the_same_amount(tc: hegel::TestCase) {
        let selection = tc.draw(selections());
        let offset_x = tc.draw(generators::integers::<usize>().max_value(1_000));
        let offset_y = tc.draw(generators::integers::<usize>().max_value(1_000));
        let shifted = selection.offset(offset_x, offset_y);

        let sorted = selection.sorted();
        let row = tc.draw(
            generators::integers::<isize>()
                .min_value(sorted.start.line.0.max(0))
                .max_value(sorted.end.line.0.max(0)),
        ) as usize;
        let col = tc.draw(generators::integers::<usize>().max_value(u16::MAX as usize));
        assert_eq!(
            selection.contains(row, col),
            shifted.contains(row + offset_y, col + offset_x),
            "cell ({}, {}) changed membership when {:?} was offset by ({}, {})",
            row,
            col,
            selection,
            offset_x,
            offset_y
        );
    }

    #[hegel::test]
    fn an_empty_selection_contains_nothing(tc: hegel::TestCase) {
        let mut selection = tc.draw(selections());
        let position = tc.draw(positions());
        selection.start = position;
        selection.end = position;
        assert!(selection.is_empty());
        let row = tc.draw(generators::integers::<usize>().max_value(200_000));
        let col = tc.draw(generators::integers::<usize>().max_value(u16::MAX as usize));
        assert!(
            !selection.contains(row, col),
            "empty selection at {:?} claimed to contain ({}, {})",
            position,
            row,
            col
        );
    }
}

use super::*;
use hegel::generators;

#[test]
fn a_list_shorter_than_the_pane_is_shown_whole() {
    assert_eq!(list_window(3, 10, Some(0)), (0, Some(0), 3));
}

#[test]
fn the_window_grows_outward_from_the_selection() {
    assert_eq!(list_window(100, 5, Some(50)), (48, Some(50), 53));
}

#[test]
fn a_selection_at_the_top_still_fills_the_window() {
    assert_eq!(list_window(100, 5, Some(0)), (0, Some(0), 5));
}

#[test]
fn a_selection_at_the_bottom_still_fills_the_window() {
    assert_eq!(list_window(100, 5, Some(99)), (95, Some(99), 100));
}

/// The row below a list is someone else's — a help line, usually — so a
/// window that overruns its allowance corrupts it.
#[test]
fn no_selection_shows_the_top_and_no_more_than_it_was_given() {
    assert_eq!(list_window(100, 5, None), (0, None, 5));
    assert_eq!(list_window(2, 5, None), (0, None, 2));
}

#[test]
fn an_empty_list_or_no_room_draws_nothing() {
    assert_eq!(list_window(0, 5, Some(0)), (0, Some(0), 0));
    assert_eq!(list_window(10, 0, Some(3)), (0, Some(3), 0));
}

#[hegel::test]
fn a_window_is_always_in_bounds_and_holds_the_selection(tc: hegel::TestCase) {
    let item_count = tc.draw(generators::integers::<usize>().max_value(200));
    let max_rows = tc.draw(generators::integers::<usize>().max_value(50));
    let selected = tc.draw(generators::integers::<usize>().max_value(200));
    let selected = if item_count == 0 {
        None
    } else {
        Some(selected % item_count)
    };

    let (start, reported, end) = list_window(item_count, max_rows, selected);

    assert!(start <= end, "window {}..{} runs backwards", start, end);
    assert!(end <= item_count, "window ran past the end of the list");
    assert!(
        end - start <= max_rows,
        "window of {} rows was given {}",
        end - start,
        max_rows
    );
    assert_eq!(reported, selected);
    if let Some(selected) = selected
        && max_rows > 0
    {
        assert!(
            start <= selected && selected < end,
            "selection {} fell outside the window {}..{}",
            selected,
            start,
            end
        );
    }
}

/// A window that leaves rows empty while items are still off screen wastes
/// the pane, which is the failure the alternating growth exists to avoid.
#[hegel::test]
fn a_window_uses_every_row_it_can(tc: hegel::TestCase) {
    let item_count = tc.draw(generators::integers::<usize>().min_value(1).max_value(200));
    let max_rows = tc.draw(generators::integers::<usize>().max_value(50));
    let selected = tc.draw(generators::integers::<usize>().max_value(200)) % item_count;

    let (start, _, end) = list_window(item_count, max_rows, Some(selected));
    assert_eq!(
        end - start,
        max_rows.min(item_count),
        "showed {} of {} items with room for {}",
        end - start,
        item_count,
        max_rows
    );
}

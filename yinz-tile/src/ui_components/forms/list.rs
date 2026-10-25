//! Which slice of a long list is on screen.
//!
//! The vertical counterpart to [`viewport`](super::viewport): that one decides
//! which columns of a line are visible, this one decides which rows of a list
//! are. Promoted from `default-plugins/strider/src/shared.rs`, where it was
//! called `calculate_list_bounds` and was already called from six places.
//!
//! Pure, so the cases that are awkward to reach in a running plugin — a list
//! shorter than the pane, a selection at either end, a pane with no room at all
//! — are testable directly.

/// The half-open row range `start..end` to draw, given a selection to keep on
/// screen.
///
/// The window grows outward from the selection, alternating up and down, so a
/// selection in the middle of a long list sits in the middle of the window and
/// one near an end still fills the available rows. The returned selection is
/// passed straight back through, which is what lets a caller write
/// `Some(i) == selected` inside its draw loop.
///
/// The range is always inside `0..item_count` and never longer than `max_rows`.
/// It is `0..0` when there is no room or nothing to show.
pub fn list_window(
    item_count: usize,
    max_rows: usize,
    selected: Option<usize>,
) -> (usize, Option<usize>, usize) {
    let Some(selected) = selected else {
        // Nothing to anchor on, so show the top of the list. strider's version
        // returned `max_rows + 1` here, which drew one row more than it was
        // given whenever a directory was visited for the first time.
        return (0, None, item_count.min(max_rows));
    };
    if item_count == 0 || max_rows == 0 {
        return (0, Some(selected), 0);
    }

    let selected = selected.min(item_count - 1);
    let mut room_in_list = max_rows;
    let mut start_index = selected;
    let mut end_index = selected + 1;
    // The selected row itself is the first thing the window spends room on.
    room_in_list = room_in_list.saturating_sub(1);
    let mut alternate = false;
    loop {
        if room_in_list == 0 {
            break;
        }
        if !alternate && start_index > 0 {
            start_index -= 1;
        } else if alternate && end_index < item_count {
            end_index += 1;
        } else if start_index > 0 {
            start_index -= 1;
        } else if end_index < item_count {
            end_index += 1;
        } else {
            break;
        }
        room_in_list -= 1;
        alternate = !alternate;
    }
    (start_index, Some(selected), end_index)
}

#[cfg(test)]
#[path = "unit/list.rs"]
mod tests;

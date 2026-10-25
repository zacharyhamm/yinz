use super::*;

fn tabs(width: usize) -> Vec<TabCell<usize>> {
    cells(
        [
            ("Agents".to_owned(), true, false, 0),
            ("Log!".to_owned(), false, true, 1),
        ],
        width,
    )
}

/// Four sample tabs, with a marker on whichever of Log and Shell is failing.
fn panel(log: bool, notes: bool, width: usize) -> Vec<String> {
    let mark = |name: &str, marked: bool| match marked {
        true => format!("{name}!"),
        false => name.to_owned(),
    };
    cells(
        [
            ("Agents".to_owned(), true, false, 0),
            ("Files".to_owned(), false, false, 1),
            (mark("Log", log), false, false, 2),
            (mark("Shell", notes), false, false, 3),
        ],
        width,
    )
    .into_iter()
    .map(|cell| cell.text)
    .collect()
}

#[test]
fn padded_cells_carry_geometry_state_and_targets() {
    let cells = tabs(20);
    assert_eq!(
        cells[0],
        TabCell {
            x: 0,
            width: 9,
            text: " Agents │".to_owned(),
            active: true,
            focused: false,
            target: 0
        }
    );
    assert_eq!(
        cells[1],
        TabCell {
            x: 9,
            width: 6,
            text: " Log!│".to_owned(),
            active: false,
            focused: true,
            target: 1
        }
    );
}

#[test]
fn narrow_cells_stay_within_the_width() {
    assert_eq!(
        tabs(2)
            .iter()
            .map(|cell| (cell.x, cell.width, cell.text.as_str(), cell.target))
            .collect::<Vec<_>>(),
        [(0, 1, "A", 0), (1, 1, "L", 1)]
    );
    assert_eq!(
        tabs(1).iter().map(|cell| cell.target).collect::<Vec<_>>(),
        [0]
    );
    assert!(tabs(0).is_empty());
}

/// An error costs no width in the padded tier, so it can never be what collapses the bar.
#[test]
fn a_marker_never_changes_the_tier() {
    let plain = panel(false, false, 31);
    assert_eq!(plain, [" Agents │", " Files │", " Log │", " Shell │"]);
    assert_eq!(
        panel(true, true, 31),
        [" Agents │", " Files │", " Log!│", " Shell!│"]
    );
}

/// Between padded and initials, whole names: the narrowest default list half still reads.
#[test]
fn a_tight_tier_keeps_whole_names() {
    assert_eq!(
        panel(false, false, 30),
        ["Agents│", "Files│", "Log│", "Shell"]
    );
    assert_eq!(
        panel(false, false, 22),
        ["Agents│", "Files│", "Log│", "Shell"]
    );
    assert_eq!(
        panel(true, true, 24),
        ["Agents│", "Files│", "Log!│", "Shell!"]
    );
    assert_eq!(
        panel(false, true, 23),
        ["Agents│", "Files│", "Log│", "Shell!"]
    );
    // Below the names, initials - and the markers too while there is room for them.
    assert_eq!(panel(false, false, 21), ["A", "F", "L", "S"]);
    assert_eq!(panel(true, true, 23), ["A", "F", "L!", "S!"]);
    assert_eq!(panel(true, false, 5), ["A", "F", "L!", "S"]);
    assert_eq!(panel(true, false, 4), ["A", "F", "L", "S"]);
}

/// The active tab keeps its bold, and only a marker is painted in the error colour - never
/// a `!` a chat title happens to end with.
#[test]
fn styling_marks_the_active_tab_and_the_error() {
    let styled = |text: &str, active: bool| {
        format!(
            "{:?}",
            super::styled(&TabCell {
                x: 0,
                width: text.width(),
                text: text.to_owned(),
                active,
                focused: false,
                target: (),
            })
        )
    };
    assert_eq!(
        styled(" Log!│", true),
        format!(
            "{:?}",
            Text::new(" Log!│")
                .color_range(2, ..)
                .error_color_range(4..=4)
        )
    );
    assert_eq!(
        styled("Shell!", false),
        format!(
            "{:?}",
            Text::new("Shell!")
                .unbold_range(..)
                .dim_range(..)
                .error_color_range(5..=5)
        )
    );
    assert_eq!(
        styled(&label("Ship it!"), false),
        format!(
            "{:?}",
            Text::new(label("Ship it!")).unbold_range(..).dim_range(..)
        )
    );
}

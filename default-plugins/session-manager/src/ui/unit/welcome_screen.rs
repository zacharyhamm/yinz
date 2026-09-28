use super::*;

#[test]
fn the_full_banner_appears_once_it_fits_beside_the_left_border() {
    // BANNER is 88 columns wide and the left border takes one more.
    assert_eq!(banner_for(88).map(|(art, _)| art), Some(MEDIUM_BANNER));
    assert_eq!(banner_for(89), Some((BANNER, 1)));
    assert_eq!(banner_for(100).map(|(art, _)| art), Some(BANNER));
    assert_eq!(banner_for(MAIN_MENU_MAX_WIDTH), Some((BANNER, 7)));
}

#[test]
fn narrower_banners_take_over_below_their_widths() {
    assert_eq!(banner_for(64), Some((MEDIUM_BANNER, 1)));
    assert_eq!(banner_for(63).map(|(art, _)| art), Some(SMALL_BANNER));
    assert_eq!(banner_for(19), Some((SMALL_BANNER, 1)));
    assert_eq!(banner_for(18), None);
}

#[test]
fn every_menu_width_gets_the_widest_banner_that_fits_between_its_borders() {
    for menu_width in 0..=MAIN_MENU_MAX_WIDTH {
        let fits = |art: &str| display_width(art) < menu_width;
        match banner_for(menu_width) {
            Some((art, column)) => {
                assert!(column >= 1, "{menu_width}: drawn over the left border");
                for line in art.lines() {
                    assert!(
                        column + line.width() <= menu_width,
                        "{menu_width}: {line:?} at {column} passes the right border"
                    );
                }
                let widest = BANNERS
                    .into_iter()
                    .filter(|other| fits(other))
                    .map(display_width)
                    .max();
                assert_eq!(widest, Some(display_width(art)), "{menu_width}");
            },
            None => assert!(!BANNERS.into_iter().any(fits), "{menu_width}"),
        }
    }
}

#[test]
fn the_widest_menu_has_room_for_the_full_banner() {
    assert_eq!(
        banner_for(main_menu_width(usize::MAX)).map(|(art, _)| art),
        Some(BANNER)
    );
}

#[test]
fn the_greeting_is_centered_between_the_borders_and_dropped_when_it_cannot_fit() {
    assert_eq!(centered_column(GREETING, MAIN_MENU_MAX_WIDTH), Some(44));
    assert_eq!(centered_column(GREETING, 14), Some(1));
    assert_eq!(centered_column(GREETING, 13), None);
}

#[test]
fn the_menu_is_centered_in_panes_wider_than_it_and_fills_narrower_ones() {
    assert_eq!(main_menu_left_border(121), 10);
    assert_eq!(main_menu_left_border(122), 10);
    assert_eq!(main_menu_left_border(MAIN_MENU_MAX_WIDTH), 0);
    assert_eq!(main_menu_left_border(50), 0);
}

#[test]
fn the_widest_menu_draws_its_banner_and_greeting_where_they_were_drawn_before() {
    // A pane 121 wide puts the menu's left border at column 10. Before the widths were
    // derived, the banner moved `x - 1 + (101 - 88) / 2` columns and the greeting went to
    // column `x + (101 - 13) / 2 - 1`, with `x` two columns right of the border.
    let pane = 121;
    let (left_border, menu_width) = (main_menu_left_border(pane), main_menu_width(pane));

    let mut banner = vec!["\u{1b}[1H".to_string()];
    banner.extend(BANNER.lines().map(|line| format!("\u{1b}[17C{line}")));
    assert_eq!(banner_lines(left_border, 0, 10, menu_width), banner);

    assert_eq!(
        banner_lines(left_border, 0, 5, menu_width),
        ["\u{1b}[3;55H\u{1b}[1mHi from Yinz!"]
    );
}

/// The zero-based column and text of each line `banner_lines` draws, skipping the cursor
/// moves that only choose a row.
fn drawn(lines: &[String]) -> Vec<(usize, &str)> {
    lines
        .iter()
        .filter_map(|line| {
            let rest = line
                .strip_prefix("\u{1b}[")
                .unwrap_or_else(|| panic!("{line:?} does not start with CSI"));
            let end = rest
                .find(|c: char| !c.is_ascii_digit() && c != ';')
                .unwrap_or_else(|| panic!("{line:?} has no final byte"));
            let (params, rest) = rest.split_at(end);
            let (final_byte, text) = rest.split_at(1);
            let number = |n: &str| -> usize { n.parse().unwrap_or_else(|_| panic!("{line:?}")) };
            match (final_byte, params.split_once(';')) {
                ("C", None) => Some((number(params), text)),
                ("H", None) => {
                    assert_eq!(text, "", "{line:?} draws without choosing a column");
                    None
                },
                ("H", Some((_, column))) => {
                    let text = text
                        .strip_prefix("\u{1b}[1m")
                        .unwrap_or_else(|| panic!("{line:?} is not bold"));
                    let column = number(column)
                        .checked_sub(1)
                        .unwrap_or_else(|| panic!("{line:?} positions at column 0"));
                    Some((column, text))
                },
                _ => panic!("{line:?} neither moves right nor positions"),
            }
        })
        .collect()
}

#[test]
fn every_pane_width_draws_between_the_menus_borders() {
    for pane in 0..=MAIN_MENU_MAX_WIDTH + 30 {
        let (left_border, menu_width) = (main_menu_left_border(pane), main_menu_width(pane));
        let right_border = left_border + menu_width;
        for rows in 0..=12 {
            let lines = banner_lines(left_border, 0, rows, menu_width);
            for (column, text) in drawn(&lines) {
                assert!(
                    column > left_border,
                    "pane {pane}, rows {rows}: {text:?} at {column} is on or left of the \
                     left border at {left_border}"
                );
                assert!(
                    column + text.width() <= right_border,
                    "pane {pane}, rows {rows}: {text:?} at {column} reaches the right border \
                     at {right_border}"
                );
            }
        }
    }
}

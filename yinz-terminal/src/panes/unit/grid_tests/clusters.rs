//! Zero-width code points stay attached to the cell before them (per-code-point emulation, as
//! Zellij with grapheme mode off), and every text path reads the whole cell.
use super::*;
use crate::panes::cluster_table;
use crate::panes::kitty_graphics::PLACEHOLDER;

const FAMILY: &str = "\u{1F469}\u{200D}\u{1F467}";

fn texts(row: &Row) -> Vec<String> {
    row.columns.iter().map(|cell| cell.text()).collect()
}

fn widths(row: &Row) -> Vec<usize> {
    row.columns.iter().map(|cell| cell.width()).collect()
}

#[test]
fn combining_marks_attach_to_the_cell_before_them() {
    let grid = create_grid_with_size_and_raw(3, 10, "e\u{301}\u{302}x".as_bytes());
    let row = &grid.viewport[0];
    assert_eq!(texts(row), ["e\u{301}\u{302}", "x"]);
    assert_eq!(widths(row), [1, 1]);
    assert_eq!(grid.cursor.x, 2);
}

#[test]
fn a_mark_after_a_wide_character_attaches_to_the_wide_cell() {
    let grid = create_grid_with_size_and_raw(3, 10, "a\u{4F60}\u{301}b".as_bytes());
    let row = &grid.viewport[0];
    assert_eq!(texts(row), ["a", "\u{4F60}\u{301}", "b"]);
    assert_eq!(widths(row), [1, 2, 1]);
}

#[test]
fn a_mark_at_a_pending_wrap_attaches_to_the_last_column() {
    let mut grid = create_grid_with_size_and_raw(3, 4, b"abcd");
    assert_eq!(grid.cursor.x, 4, "the wrap is pending");
    feed_bytes(&mut grid, "\u{301}".as_bytes());
    assert_eq!(texts(&grid.viewport[0]), ["a", "b", "c", "d\u{301}"]);
    feed_bytes(&mut grid, b"e");
    assert_eq!(texts(&grid.viewport[1]), ["e"]);
}

#[test]
fn a_mark_with_no_cell_before_it_is_dropped() {
    let grid = create_grid_with_size_and_raw(3, 10, "\u{301}a\r\n\x1b[5C\u{301}".as_bytes());
    assert_eq!(texts(&grid.viewport[0]), ["a"]);
    assert!(grid.viewport[1]
        .columns
        .iter()
        .all(|cell| !cell.has_attached()));
}

#[test]
fn joiners_attach_and_width_carrying_members_keep_their_own_cells() {
    let grid = create_grid_with_size_and_raw(
        3,
        20,
        format!("{FAMILY}\u{1F1E9}\u{1F1EA}\u{26A0}\u{FE0F}\u{1F44D}\u{1F3FD}").as_bytes(),
    );
    let row = &grid.viewport[0];
    assert_eq!(
        texts(row),
        [
            "\u{1F469}\u{200D}",
            "\u{1F467}",
            "\u{1F1E9}",
            "\u{1F1EA}",
            "\u{26A0}\u{FE0F}",
            "\u{1F44D}",
            "\u{1F3FD}",
        ]
    );
    assert_eq!(widths(row), [2, 2, 1, 1, 1, 2, 2]);
}

#[test]
fn rep_repeats_the_whole_cluster() {
    let grid = create_grid_with_size_and_raw(3, 10, "e\u{301}\x1b[2b".as_bytes());
    assert_eq!(texts(&grid.viewport[0]), ["e\u{301}"; 3]);
}

#[test]
fn placeholder_diacritics_win_and_other_marks_never_join_a_placeholder() {
    let grid = create_grid_with_size_and_raw(
        3,
        10,
        format!("e\u{305}{PLACEHOLDER}\u{305}\u{30D}{PLACEHOLDER}\u{301}").as_bytes(),
    );
    let row = &grid.viewport[0];
    assert_eq!(
        texts(row),
        [
            "e\u{305}".to_string(),
            PLACEHOLDER.to_string(),
            PLACEHOLDER.to_string()
        ]
    );
    let mark = row.placeholder_mark_at(1).unwrap();
    assert_eq!(
        (mark.row_in_image, mark.column_in_image),
        (Some(0), Some(1))
    );
}

#[test]
fn every_text_path_reads_whole_cells() {
    let mut grid = create_grid_with_size_and_raw(
        4,
        20,
        format!("a{FAMILY}e\u{301}\r\n\x1b[31mx\u{301}\u{302}\x1b[0m").as_bytes(),
    );
    let first = format!("a{FAMILY}e\u{301}");
    assert!(grid.dump_screen(true).contains(&first));
    assert!(grid.dump_screen_with_ansi(true).contains("x\u{301}\u{302}"));
    let contents = grid.pane_contents(false, None);
    assert_eq!(contents.viewport[0].trim_end(), first);
    let with_ansi = grid.pane_contents_with_ansi(false, None);
    assert!(
        with_ansi.viewport[0].contains(&first),
        "{:?}",
        with_ansi.viewport
    );
    assert!(with_ansi.viewport[1].contains("x\u{301}\u{302}"));

    grid.start_selection(&Position::new(0, 0));
    grid.end_selection(&Position::new(0, 7));
    assert_eq!(grid.get_selected_text().unwrap(), first);
}

#[test]
fn a_trailing_space_carrying_marks_is_kept_by_every_text_path() {
    let grid = create_grid_with_size_and_raw(4, 20, "a \u{301}\r\nb   ".as_bytes());
    let marked = "a \u{301}";
    assert_eq!(grid.viewport[0].columns[1].text(), " \u{301}");
    assert!(grid.dump_screen(true).contains(marked));
    assert!(
        grid.dump_screen_with_ansi(true).contains(marked),
        "{:?}",
        grid.dump_screen_with_ansi(true)
    );
    assert_eq!(
        grid.pane_contents(false, None).viewport[0].trim_end(),
        marked
    );
    let with_ansi = grid.pane_contents_with_ansi(false, None);
    assert!(
        with_ansi.viewport[0].contains(marked),
        "{:?}",
        with_ansi.viewport
    );
    // Unmarked trailing spaces are still trimmed.
    assert!(
        !with_ansi.viewport[1].contains("b "),
        "{:?}",
        with_ansi.viewport
    );
}

#[test]
fn copied_text_with_attached_code_points_finds_its_source() {
    let line = format!("cafe\u{301} \u{26A0}\u{FE0F} warn {FAMILY} end");
    let mut grid = create_grid_with_size_and_raw(4, 40, line.as_bytes());
    grid.start_selection(&Position::new(0, 0));
    grid.end_selection(&Position::new(0, 25));
    let copied = grid.get_selected_text().unwrap();
    assert_eq!(copied, line);

    for needle in [
        copied.as_str(),
        "cafe\u{301}",
        "\u{26A0}\u{FE0F} warn",
        FAMILY,
        "cafe",
        "\u{26A0} warn",
        "\u{1F469}\u{1F467}",
    ] {
        grid.set_search_string(needle);
        assert_eq!(grid.search_results.selections.len(), 1, "{needle:?}");
    }
    grid.set_search_string(&copied);
    let hit = &grid.search_results.selections[0];
    assert_eq!(hit.start, Position::new(0, 0));
    assert_eq!(hit.end, Position::new(0, 20));

    // A needle of nothing but attached code points matches nothing.
    grid.set_search_string("\u{301}\u{200D}");
    assert!(grid.search_results.selections.is_empty());
}

#[test]
fn overwriting_and_erasing_clustered_cells_frees_their_entries() {
    let before = cluster_table::live_entries();
    let mut grid = create_grid_with_size_and_raw(
        3,
        20,
        "e\u{301}\u{302}\u{303}x\u{301}\u{302}\u{304}".as_bytes(),
    );
    assert_eq!(cluster_table::live_entries(), before + 2);
    feed_bytes(&mut grid, b"\x1b[1;1Hq");
    assert_eq!(cluster_table::live_entries(), before + 1);
    feed_bytes(&mut grid, b"\x1b[2J");
    drop(grid);
    assert_eq!(cluster_table::live_entries(), before);
}

#[test]
fn clustered_rows_reflow_without_losing_marks() {
    let mut grid = create_grid_with_size_and_raw(3, 6, format!("ab{FAMILY}e\u{301}").as_bytes());
    grid.change_size(3, 4);
    let joined: String = grid
        .viewport
        .iter()
        .flat_map(|row| row.columns.iter())
        .map(|cell| cell.text())
        .collect();
    assert!(
        joined.contains(FAMILY) && joined.contains("e\u{301}"),
        "{joined:?}"
    );
}

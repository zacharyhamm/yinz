use super::*;

// -- table --------------------------------------------------------------

#[test]
fn table_is_a_markdown_table() {
    let rows = vec![
        vec!["1".to_owned(), "short".to_owned()],
        vec!["22".to_owned(), "longer".to_owned()],
    ];
    assert_eq!(
        table(&["id", "title"], &rows),
        "| id | title |\n| --- | --- |\n| 1 | short |\n| 22 | longer |\n"
    );
}

/// Columns are the renderer's job now, so this only has to hand it a cell per
/// column: a row short of cells is padded rather than left ragged, or every row
/// after it would be read into the wrong column.
#[test]
fn a_short_row_still_fills_its_columns() {
    let rows = vec![vec!["1".to_owned()]];
    assert_eq!(
        table(&["id", "title"], &rows),
        "| id | title |\n| --- | --- |\n| 1 |  |\n"
    );
}

/// The fence used to be what stopped a hostile title restyling the transcript.
/// Nothing is fenced now, so every cell goes through `inline` - and a pipe in a
/// title would otherwise open a cell of its own.
#[test]
fn a_cell_is_escaped_now_that_nothing_fences_it() {
    let rows = vec![vec!["a|b".to_owned(), "# **x**".to_owned()]];
    let rendered = table(&["id", "title"], &rows);
    assert!(rendered.contains(r"a\|b"), "{rendered}");
    assert!(rendered.contains(r"\# \*\*x\*\*"), "{rendered}");
}

#[test]
fn table_of_no_rows_is_nothing_at_all() {
    assert_eq!(table(&["id"], &[]), "");
}

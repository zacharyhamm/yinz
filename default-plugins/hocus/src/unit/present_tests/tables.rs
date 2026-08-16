use super::*;

// -- table --------------------------------------------------------------

#[test]
fn table_aligns_its_columns() {
    let rows = vec![
        vec!["1".to_owned(), "short".to_owned()],
        vec!["22".to_owned(), "longer".to_owned()],
    ];
    assert_eq!(
        table(&["id", "title"], &rows),
        "```\nid  title\n1   short\n22  longer\n```\n"
    );
}

/// A CJK title is two columns per character; measuring it in `char`s would
/// skew every column to its right.
#[test]
fn table_measures_wide_characters_by_display_width() {
    let rows = vec![
        vec!["日本".to_owned(), "a".to_owned()],
        vec!["ab".to_owned(), "b".to_owned()],
    ];
    let rendered = table(&["name", "x"], &rows);
    for line in rendered.lines().filter(|line| !line.starts_with("```")) {
        let column = line.find(['a', 'b', 'x']).map(|_| ());
        assert!(column.is_some(), "{line:?}");
    }
    assert!(rendered.contains("日本  a"), "{rendered}");
}

#[test]
fn table_of_no_rows_is_nothing_at_all() {
    assert_eq!(table(&["id"], &[]), "");
}

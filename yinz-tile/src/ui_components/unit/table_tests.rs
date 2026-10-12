use super::*;

#[test]
fn tables_emit_one_dcs_terminator() {
    let table = Table::new().add_row(vec!["a", "b"]).add_row(vec!["c", "d"]);
    let ordinary = serialize_table(&table);
    assert_eq!(ordinary, format!("\x1bPytable;{}", table.serialize()));
    assert_eq!(ordinary.matches("\x1b\\").count(), 1);
    for (width, height, coordinates) in [
        (Some(10), Some(4), "1/2/10/4"),
        (Some(10), None, "1/2/10/"),
        (None, Some(4), "1/2//4"),
        (None, None, "1/2//"),
    ] {
        let serialized = serialize_table_with_coordinates(&table, 1, 2, width, height);
        assert_eq!(serialized.matches("\x1b\\").count(), 1);
        assert_eq!(
            serialized,
            format!("\x1bPytable;{coordinates};{}", table.serialize())
        );
        assert!(serialized.ends_with("\x1b\\"));
    }
}

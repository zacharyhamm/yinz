use super::*;

#[test]
fn visible_coordinates_preserve_array_order_and_maximum_values() {
    let cursor = CellCoordinates::new(Column::new(7), Row::new(3));
    assert_eq!(serde_json::to_string(&cursor).unwrap(), "[7,3]");
    assert_eq!(
        serde_json::from_str::<CellCoordinates>("[7,3]").unwrap(),
        cursor
    );
    let max = CellCoordinates::from((usize::MAX, usize::MAX));
    let json = format!("[{0},{0}]", usize::MAX);
    assert_eq!(serde_json::to_string(&max).unwrap(), json);
    assert_eq!(serde_json::from_str::<CellCoordinates>(&json).unwrap(), max);
    assert!(serde_json::from_str::<CellCoordinates>("[0,-1]").is_err());
    assert!(serde_json::from_str::<CellCoordinates>("[-1,0]").is_err());
    assert_eq!(
        serde_json::to_string(&RowCount::new(usize::MAX)).unwrap(),
        usize::MAX.to_string()
    );
    assert_eq!(
        serde_json::to_string(&ColumnCount::new(usize::MAX)).unwrap(),
        usize::MAX.to_string()
    );
}

#[test]
fn narrowing_rejects_overflow_and_visible_rows_reject_scrollback() {
    assert_eq!(Line::try_from(-7_i64).unwrap().get(), -7);
    assert!(Row::try_from(-7_i64).is_err());
    assert!(Column::try_from(-1_i64).is_err());
    assert!(RowCount::try_from(-1_i64).is_err());
    assert!(ColumnCount::try_from(-1_i64).is_err());
    assert_eq!(
        u16::try_from(Column::new(u16::MAX as usize)).unwrap(),
        u16::MAX
    );
    assert!(u16::try_from(Column::new(u16::MAX as usize + 1)).is_err());
    if usize::BITS > 32 {
        assert!(u32::try_from(RowCount::new(usize::MAX)).is_err());
        assert!(u32::try_from(ColumnCount::new(usize::MAX)).is_err());
        assert!(i32::try_from(Line::new(isize::MAX)).is_err());
    }
}

#[test]
fn signed_lines_and_unsigned_columns_keep_their_json_values() {
    for value in [isize::MIN, -1, 0, isize::MAX] {
        let json = value.to_string();
        assert_eq!(serde_json::to_string(&Line::new(value)).unwrap(), json);
        assert_eq!(
            serde_json::from_str::<Line>(&json).unwrap(),
            Line::new(value)
        );
    }
    for value in [0, usize::MAX] {
        let json = value.to_string();
        assert_eq!(serde_json::to_string(&Column::new(value)).unwrap(), json);
        assert_eq!(
            serde_json::from_str::<Column>(&json).unwrap(),
            Column::new(value)
        );
    }
    assert!(serde_json::from_str::<Column>("-1").is_err());
}

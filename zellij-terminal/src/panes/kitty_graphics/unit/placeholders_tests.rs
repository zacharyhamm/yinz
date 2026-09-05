use super::*;

#[test]
fn the_diacritic_table_is_sorted_and_free_of_duplicates() {
    // `diacritic_value` binary searches the table itself rather than a second, reordered copy, so
    // this is the invariant that makes the lookup correct rather than merely fast.
    for pair in ROWCOLUMN_DIACRITICS.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{:?} and {:?} are out of order",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn every_diacritic_decodes_back_to_its_own_index() {
    for (index, character) in ROWCOLUMN_DIACRITICS.iter().enumerate() {
        assert_eq!(diacritic_value(*character), Some(index as u16));
    }
}

#[test]
fn the_table_matches_the_fixture_the_conformance_suite_encodes_with() {
    // The suite's `placeholder_cells` builds its input from its own prefix of the table; if the
    // two ever drift, every placeholder test would be encoding values this code decodes
    // differently.
    let fixture = crate::panes::kitty_graphics::test_support::ROWCOLUMN_DIACRITICS;
    assert_eq!(fixture.as_slice(), &ROWCOLUMN_DIACRITICS[..fixture.len()]);
}

#[test]
fn a_combining_mark_outside_the_table_encodes_nothing() {
    // U+0300 COMBINING GRAVE ACCENT is a combining mark, but not one of the 297. Following a
    // placeholder cell with it must not be read as "row 0".
    assert_eq!(diacritic_value('\u{0300}'), None);
    assert_eq!(diacritic_value('a'), None);
}

#[test]
fn marks_are_taken_as_row_then_column_then_id_byte() {
    let mut mark = PlaceholderMark::new(3);
    assert_eq!(mark.column, 3);
    mark.push_value(7);
    assert_eq!(mark.row_in_image, Some(7));
    mark.push_value(8);
    assert_eq!(mark.column_in_image, Some(8));
    mark.push_value(9);
    assert_eq!(mark.id_most_significant_byte, Some(9));
    // A fourth mark is not part of the encoding.
    mark.push_value(10);
    assert_eq!(
        mark,
        PlaceholderMark {
            column: 3,
            row_in_image: Some(7),
            column_in_image: Some(8),
            id_most_significant_byte: Some(9),
        }
    );
}

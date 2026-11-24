use crate::panes::cluster_table;
use crate::panes::terminal_character::{
    RcCharacterStyles, TerminalCharacter, DEFAULT_STYLES, EMPTY_TERMINAL_CHARACTER,
};
use crate::panes::AnsiCode;

#[cfg(target_arch = "x86_64")]
#[test]
fn terminal_character_stays_sixteen_bytes() {
    assert_eq!(std::mem::size_of::<TerminalCharacter>(), 16);
}

fn table_cell(width_two: bool) -> TerminalCharacter {
    let mut cell = TerminalCharacter::new(if width_two { '\u{1F468}' } else { 'e' });
    assert!(cell.attach('\u{0301}'));
    assert!(cell.attach('\u{0302}'));
    cell
}

#[test]
fn packed_fields_read_back_unchanged() {
    let mut cell = table_cell(true);
    assert_eq!(cell.width(), 2);
    assert!(!cell.is_implicit_blank());
    assert_eq!(cell.text(), "\u{1F468}\u{0301}\u{0302}");

    let mut blank = EMPTY_TERMINAL_CHARACTER;
    assert!(blank.is_implicit_blank());
    assert_eq!(blank.width(), 1);
    assert!(!blank.has_attached());
    assert!(blank.attach('\u{0301}'));
    assert!(blank.attach('\u{0302}'));
    assert!(blank.is_implicit_blank());
    assert_eq!(blank.width(), 1);
    assert_eq!(blank.text(), " \u{0301}\u{0302}");

    cell.mark_implicit_blank();
    assert!(!cell.is_implicit_blank(), "only blanks become implicit");
    assert_eq!(cell.width(), 2);
}

#[test]
fn the_largest_id_keeps_width_and_flag() {
    let mut cell = TerminalCharacter::new('\u{1F468}');
    cell.mark_implicit_blank();
    let mut blank = EMPTY_TERMINAL_CHARACTER;
    blank.styles = cell.styles.clone();
    for (mut cell, explicit) in [(cell, true), (blank, false)] {
        let width = cell.width();
        // No such entry exists, so retain and release on clone and drop are no-ops.
        cell.set_table_id_for_tests(cluster_table::MAX_ID);
        assert_eq!(cell.table_id_for_tests(), Some(cluster_table::MAX_ID));
        assert_eq!(cell.width(), width);
        assert_eq!(cell.is_implicit_blank(), !explicit);
        assert!(cell.has_attached());
    }
    let mut wide = TerminalCharacter::new('\u{4F60}');
    wide.set_table_id_for_tests(cluster_table::MAX_ID);
    assert_eq!(wide.width(), 2);
    assert!(!wide.is_implicit_blank());
}

#[test]
fn display_and_debug_print_the_whole_cell() {
    let cell = table_cell(false);
    assert_eq!(format!("{cell}"), "e\u{0301}\u{0302}");
    assert_eq!(format!("{cell:?}"), "e\u{0301}\u{0302}");
}

#[test]
fn serde_round_trips_attached_text_and_restores_old_snapshots() {
    let styles: RcCharacterStyles = DEFAULT_STYLES
        .foreground(Some(AnsiCode::ColorIndex(3)))
        .into();
    let mut cell = TerminalCharacter::new_styled('a', styles);
    assert!(cell.attach('\u{0301}'));
    let json = serde_json::to_string(&cell).unwrap();
    let restored: TerminalCharacter = serde_json::from_str(&json).unwrap();
    assert!(restored == cell);
    assert_eq!(restored.text(), "a\u{0301}");

    let plain = TerminalCharacter::new('b');
    let json = serde_json::to_value(&plain).unwrap();
    assert!(
        json.get("attached").is_none(),
        "plain cells keep the old format"
    );

    // A format-2 snapshot from before attached text restores unchanged.
    let old = serde_json::json!({
        "character": 'c',
        "styles": serde_json::to_value(RcCharacterStyles::default()).unwrap(),
        "width": 1,
        "is_explicit": false,
    });
    let restored: TerminalCharacter = serde_json::from_value(old).unwrap();
    assert_eq!(restored.text(), "c");
    assert_eq!(restored.width(), 1);

    // Out-of-range widths are clamped instead of spilling into the flag bit.
    let wide = serde_json::json!({
        "character": 'd',
        "styles": serde_json::to_value(RcCharacterStyles::default()).unwrap(),
        "width": 200,
        "is_explicit": true,
        "attached": "\u{0301}",
    });
    let restored: TerminalCharacter = serde_json::from_value(wide).unwrap();
    assert_eq!(restored.width(), 127);
    assert_eq!(restored.text(), "d\u{0301}");
}

#[test]
fn restoring_at_the_table_limit_keeps_the_cell() {
    let json = serde_json::to_value(table_cell(false)).unwrap();
    cluster_table::set_limit_for_tests(0);
    let restored: TerminalCharacter = serde_json::from_value(json).unwrap();
    // The inline mark fits without the table; the second needs it and is dropped.
    assert_eq!(restored.text(), "e\u{0301}");
    cluster_table::set_limit_for_tests(cluster_table::DEFAULT_BYTE_LIMIT);
}

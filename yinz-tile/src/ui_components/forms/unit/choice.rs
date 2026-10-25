use super::*;

fn key(bare_key: BareKey) -> KeyWithModifier {
    KeyWithModifier::new(bare_key)
}

#[test]
fn checkbox_can_be_set_and_toggled() {
    let mut checkbox = Checkbox::new(false);
    checkbox.set_checked(true);
    assert!(checkbox.is_checked());
    checkbox.toggle();
    assert!(!checkbox.is_checked());
}

#[test]
fn radio_group_selects_by_index_or_label() {
    let mut radio = RadioGroup::new(["one", "two"]);
    assert_eq!(radio.selected_option(), Some("one"));
    assert!(radio.select_index(1));
    assert_eq!(radio.selected_option(), Some("two"));
    assert!(radio.select_option("one"));
    assert!(!radio.select_option("missing"));
}

#[test]
fn radio_arrows_wrap() {
    let mut radio = RadioGroup::new(["one", "two", "three"]);
    assert!(radio.handle_key(key(BareKey::Left)));
    assert_eq!(radio.selected_option(), Some("three"));
    assert!(radio.handle_key(key(BareKey::Right)));
    assert_eq!(radio.selected_option(), Some("one"));
}

#[test]
fn empty_radio_group_has_no_selection() {
    let radio = RadioGroup::new(Vec::<String>::new());
    assert_eq!(radio.selected_index(), None);
    assert_eq!(radio.selected_option(), None);
}

#[test]
fn the_checkbox_marker_rect_is_where_the_marker_was_drawn() {
    let mut checkbox = Checkbox::new(true);
    assert_eq!(checkbox.last_marker, None);
    // "Frames: " is eight columns, so the marker starts eight past the row.
    checkbox.render_row(&mut RenderBuffer::default(), "Frames: ", 4, 2, 20, false);
    assert_eq!(checkbox.last_marker, Some(Rect::new(12, 2, 3, 1)));
    checkbox.clear_rendered();
    assert_eq!(checkbox.last_marker, None);
}

#[test]
fn a_checkbox_with_no_room_for_its_marker_records_none() {
    let mut checkbox = Checkbox::new(false);
    checkbox.render_row(&mut RenderBuffer::default(), "Frames: ", 0, 0, 8, false);
    assert_eq!(checkbox.last_marker, None);
}

#[test]
fn hover_state_reports_whether_it_changed() {
    let mut checkbox = Checkbox::new(false);
    assert!(checkbox.set_hovered(true));
    assert!(!checkbox.set_hovered(true));

    let mut radio = RadioGroup::new(["one", "two"]);
    assert!(radio.set_hovered(Some(1)));
    assert!(!radio.set_hovered(Some(1)));
    assert!(radio.set_hovered(None));
}

#[test]
fn narrow_unicode_radio_options_stay_inside_the_row() {
    let mut radio = RadioGroup::new(["日本語", "English"]);
    radio.render_row(
        &mut RenderBuffer::default(),
        "Mode: ",
        4,
        2,
        12,
        true,
        false,
    );
    assert!(radio
        .last_option_rects
        .iter()
        .all(|(_, rect)| rect.x + rect.columns <= 16));
}

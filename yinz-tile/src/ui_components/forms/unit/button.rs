use super::*;
use hegel::generators;

#[test]
fn a_button_that_never_rendered_contains_nothing() {
    let button = Button::new("Save");
    assert!(!button.contains(0, 0));
    assert_eq!(button.last_rendered(), None);
}

#[test]
fn a_disabled_button_does_not_take_clicks() {
    let mut button = Button::new("Save").disabled(true);
    button.render(0, 0, None);
    assert!(!button.contains(1, 0), "disabled button swallowed a click");
}

/// Half-open bounds: adjacent buttons must not both claim a column.
#[test]
fn adjacent_buttons_do_not_overlap() {
    let mut first = Button::new("A");
    let mut second = Button::new("B");
    let width = first.render(0, 0, None);
    second.render(width, 0, None);

    for x in 0..width {
        assert!(first.contains(x, 0), "first should own column {}", x);
        assert!(!second.contains(x, 0), "second also claimed column {}", x);
    }
    assert!(second.contains(width, 0));
}

#[test]
fn hovering_swaps_the_label() {
    let mut button = Button::new("Save").hover_label("Save it!");
    assert_eq!(button.visible_label(), "Save");
    assert!(button.set_hovered(true));
    assert_eq!(button.visible_label(), "Save it!");
    assert!(!button.set_hovered(true), "no change should report false");
}

/// The rectangle recorded at render time is what clicks are tested against,
/// so it has to agree with where the button was actually drawn.
#[hegel::test]
fn hit_testing_agrees_with_the_recorded_rectangle(tc: hegel::TestCase) {
    let label = tc.draw(generators::text());
    let x = tc.draw(generators::integers::<usize>().max_value(200));
    let y = tc.draw(generators::integers::<usize>().max_value(200));

    let mut button = Button::new(&label);
    let width = button.render(x, y, None);
    let rect = button.last_rendered().expect("render records a rectangle");

    assert_eq!(rect, Rect::new(x, y, width, 1));
    assert!(!button.contains(x.saturating_sub(1), y) || x == 0);
    assert!(
        !button.contains(x + width, y),
        "claimed past its right edge"
    );
    assert!(!button.contains(x, y + 1), "claimed a row it did not draw");
    if width > 0 {
        assert!(button.contains(x, y));
    }
}

#[hegel::test]
fn a_button_never_renders_wider_than_its_limit(tc: hegel::TestCase) {
    let label = tc.draw(generators::text());
    let max = tc.draw(generators::integers::<usize>().max_value(40));
    let mut button = Button::new(&label);
    let width = button.render(0, 0, Some(max));
    assert!(width <= max, "drew {} columns, limit was {}", width, max);
}

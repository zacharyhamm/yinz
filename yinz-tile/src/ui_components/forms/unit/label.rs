use super::*;
use hegel::generators;

#[test]
fn a_label_keeps_its_content() {
    let label = Label::new("Name");
    assert_eq!(label.content(), "Name");
    assert_eq!(label.width(), 4);
}

#[test]
fn truncation_is_by_column_not_by_char() {
    // Three CJK characters are six columns wide, so four columns fit two.
    let label = Label::new("日本語");
    assert_eq!(label.to_text(Some(4)).content(), "日本");
}

/// A label that overflows the width it was handed corrupts the row beside
/// it, so this is the one property that matters.
#[hegel::test]
fn a_truncated_label_never_exceeds_its_width(tc: hegel::TestCase) {
    let content = tc.draw(generators::text());
    let max = tc.draw(generators::integers::<usize>().max_value(80));
    let rendered = Label::new(&content).to_text(Some(max));
    assert!(
        rendered.content().width() <= max,
        "{:?} rendered {:?}, wider than {} columns",
        content,
        rendered.content(),
        max
    );
}

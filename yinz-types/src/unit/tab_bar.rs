use super::*;

#[test]
fn overflow_preserves_targets_and_unicode_cell_geometry_at_every_width() {
    let labels = [" Agents │", " 任務 │", " e\u{301} │", " 👩‍💻 │"].map(str::to_owned);
    for width in 0..50 {
        for keep in 0..labels.len() {
            let cells = overflow(&labels, width, 0, Some(keep), keep);
            let mut x = 0;
            for cell in &cells {
                assert_eq!(cell.x, x);
                assert_eq!(
                    cell.width,
                    cell.text
                        .chars()
                        .map(|ch| ch.width().unwrap_or(0))
                        .sum::<usize>()
                );
                assert!(cell.width > 0);
                x += cell.width;
                assert!(x <= width);
            }
            assert_eq!(
                cells
                    .iter()
                    .any(|cell| cell.target == TabTarget::Tab(keep) && cell.focused),
                width > 0
            );
        }
    }
    let cells = overflow(&labels, 18, 0, None, 2);
    assert_eq!(cells.first().unwrap().target, TabTarget::Reveal(1));
    assert!(cells.iter().any(|cell| cell.target == TabTarget::Tab(2)));
}

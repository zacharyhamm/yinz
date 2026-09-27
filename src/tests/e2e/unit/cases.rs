use super::account_for_races_in_snapshot;

#[test]
fn clock_normalization_preserves_alignment_for_both_day_widths() {
    for trailing in ["", "   "] {
        let expected = format!(
            "pane content\n Ctrl + q {:>24}{trailing}\nnext line\n",
            "00:00  Mon 01 Jan"
        );
        for day in [1, 9, 10, 31] {
            let clock = format!("12:34  Thu {day} Oct");
            let snapshot = format!("pane content\n Ctrl + q {clock:>24}{trailing}\nnext line\n");
            let normalized = account_for_races_in_snapshot(snapshot.clone());
            assert_eq!(normalized, expected, "day {day}, trailing {trailing:?}");
            assert_eq!(normalized.len(), snapshot.len());
        }
    }
}

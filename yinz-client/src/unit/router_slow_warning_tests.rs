use super::*;

#[test]
fn threshold_suppression_peak_and_final_summary() {
    let start = Instant::now();
    let mut warnings = SlowWarnings::default();
    assert_eq!(warnings.record(start, Duration::from_millis(99)), None);
    assert_eq!(
        warnings.record(start, Duration::from_millis(100)),
        Some((0, Duration::ZERO))
    );
    assert_eq!(
        warnings.record(
            start + Duration::from_millis(100),
            Duration::from_millis(150)
        ),
        None
    );
    assert_eq!(
        warnings.record(
            start + Duration::from_millis(200),
            Duration::from_millis(120)
        ),
        None
    );
    assert_eq!(warnings.finish(), Some((2, Duration::from_millis(150))));
    assert_eq!(
        warnings.record(start + Duration::from_secs(1), Duration::from_millis(110)),
        Some((2, Duration::from_millis(150)))
    );
    assert_eq!(warnings.finish(), None);
}

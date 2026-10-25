use super::*;

#[test]
fn the_epoch_is_a_thursday() {
    let at = civil_from_epoch_ms(yinz_types::units::SignedUnixMillis::new(0), 0);
    assert_eq!((at.year, at.month, at.day), (1970, 1, 1));
    assert_eq!(WEEKDAY_NAMES[at.weekday], "Thu");
}

#[test]
fn a_negative_offset_can_walk_back_over_midnight() {
    // 1970-01-01 00:30:00 UTC in UTC-4 is 1969-12-31 20:30.
    let at = civil_from_epoch_ms(
        yinz_types::units::SignedUnixMillis::new(30 * 60 * 1000),
        -4 * 3600,
    );
    assert_eq!((at.year, at.month, at.day), (1969, 12, 31));
    assert_eq!((at.hour, at.minute), (20, 30));
    assert_eq!(WEEKDAY_NAMES[at.weekday], "Wed");
}

#[test]
fn leap_day_is_a_day() {
    // 2024-02-29T12:00:00Z
    let at = civil_from_epoch_ms(
        yinz_types::units::SignedUnixMillis::new(1_709_208_000_000),
        0,
    );
    assert_eq!((at.year, at.month, at.day), (2024, 2, 29));
    assert_eq!(MONTH_NAMES[at.month as usize - 1], "Feb");
}

#[test]
fn wall_clock_states_its_offset() {
    assert_eq!(
        wall_clock(
            yinz_types::units::SignedUnixMillis::new(1_709_208_000_000),
            0
        ),
        "2024-02-29 12:00:00 +00:00"
    );
    assert_eq!(
        wall_clock(
            yinz_types::units::SignedUnixMillis::new(1_709_208_000_000),
            -4 * 3600 - 1800
        ),
        "2024-02-29 07:30:00 -04:30"
    );
}

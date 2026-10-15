//! Civil-calendar arithmetic shared by plugin renderers.
//! The host supplies an explicit UTC offset through `GetLocalUtcOffset`; calendar
//! conversion stays deterministic and does not read the process timezone.

/// Now, as an epoch millisecond.
pub fn now_ms() -> SignedUnixMillis {
    SignedUnixMillis::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or_default(),
    )
}

/// Days since 1970-01-01 as a civil `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days` — the shifted-era algorithm the C++ committee adopted —
/// transcribed rather than invented, because it is exact for every day in the proleptic
/// Gregorian calendar and needs no table.
///
/// The era shift moves the epoch to 0000-03-01, which puts February — the month whose length
/// moves — at the end of a year rather than in the middle of one, and that is the whole trick:
/// the leap day is then the last day of the last era-year and needs no special case.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = match mp < 10 {
        true => mp + 3,
        false => mp - 9,
    };
    (year + i64::from(month <= 2), month, day)
}

/// The local calendar broken out of an epoch millisecond: `(year, month, day, hour, minute,
/// second, weekday)`, where weekday is 0 = Sunday.
///
/// `utc_offset_secs` is seconds east of UTC, which only the host can say — see
/// the `GetLocalUtcOffset` host command. `0` means UTC, which is the
/// honest answer for a caller that could not ask rather than a wrong one.
pub fn civil_from_epoch_ms(epoch_ms: SignedUnixMillis, utc_offset_secs: i32) -> CivilTime {
    let local_ms = epoch_ms
        .get()
        .saturating_add(i64::from(utc_offset_secs).saturating_mul(1000));
    // Euclidean throughout: a pre-epoch instant, or one an offset pushes before it, has to
    // floor rather than truncate towards zero, or the day comes out one too late and the
    // time of day negative.
    let days = local_ms.div_euclid(86_400_000);
    let ms_of_day = local_ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    CivilTime {
        year,
        month,
        day,
        hour: ms_of_day / 3_600_000,
        minute: ms_of_day / 60_000 % 60,
        second: ms_of_day / 1000 % 60,
        // 1970-01-01 was a Thursday, which is 4 counting Sunday as 0.
        weekday: (days + 4).rem_euclid(7) as usize,
    }
}

/// What [`civil_from_epoch_ms`] answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilTime {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    /// 0 = Sunday.
    pub weekday: usize,
}

/// Three-letter weekday names, indexed by [`CivilTime::weekday`].
pub const WEEKDAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Three-letter month names, indexed by `month - 1`.
pub const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// An epoch millisecond as the wall clock read where the session runs:
/// `"2026-09-04 14:32:07 -04:00"`.
///
/// Everything a relative age cannot answer: which day, and which minute of it.
pub fn wall_clock(epoch_ms: SignedUnixMillis, utc_offset_secs: i32) -> String {
    let at = civil_from_epoch_ms(epoch_ms, utc_offset_secs);
    let (sign, offset) = match utc_offset_secs < 0 {
        true => ('-', -i64::from(utc_offset_secs)),
        false => ('+', i64::from(utc_offset_secs)),
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {sign}{:02}:{:02}",
        at.year,
        at.month,
        at.day,
        at.hour,
        at.minute,
        at.second,
        offset / 3600,
        offset / 60 % 60,
    )
}

use yinz_types::units::SignedUnixMillis;

#[cfg(test)]
mod tests {
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
}

use super::local_time;
use yinz_types::units::SignedUnixMillis;

#[test]
fn local_dates_use_the_current_timezone_and_its_dst_abbreviation() {
    const TEST: &str = "yinz_exports::plugins::local_time_tests::local_dates_use_the_current_timezone_and_its_dst_abbreviation";
    if let Ok(zone) = std::env::var("YINZ_TEST_DATE_ZONE") {
        let expected = match zone.as_str() {
            "America/Chicago" => ["Jan 15, 2026, 12:32 PM CST", "Jul 15, 2026, 1:32 PM CDT"],
            "Asia/Kolkata" => ["Jan 16, 2026, 12:02 AM IST", "Jul 16, 2026, 12:02 AM IST"],
            _ => ["Jan 15, 2026, 6:32 PM UTC", "Jul 15, 2026, 6:32 PM UTC"],
        };
        for (date, expected) in ["2026-01-15T18:32:07Z", "2026-07-15T18:32:07Z"]
            .into_iter()
            .zip(expected)
        {
            let at = chrono::DateTime::parse_from_rfc3339(date)
                .unwrap()
                .timestamp_millis();
            assert_eq!(local_time(SignedUnixMillis::new(at)).unwrap(), expected);
        }
        return;
    }
    // A fresh process keeps TZ changes out of other tests and libc's timezone cache.
    for zone in ["America/Chicago", "Asia/Kolkata", "UTC"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env("TZ", zone)
            .env("LC_ALL", "C")
            .env("YINZ_TEST_DATE_ZONE", zone)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{zone}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}

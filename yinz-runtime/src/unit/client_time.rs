use super::*;

fn at(timezone: &str, instant: &str) -> String {
    ClientTimeZone::from_client(Some(timezone))
        .timestamp(Some(instant.parse::<Timestamp>().unwrap().as_millisecond()))
}

#[test]
fn friendly_dates_follow_client_timezone_across_midnight_and_dst() {
    assert_eq!(
        at("America/Chicago", "2026-10-30T02:00:00Z"),
        "Oct. 29th, 2026, 9pm"
    );
    assert_eq!(
        at("Asia/Kolkata", "2026-10-30T02:00:00Z"),
        "Oct. 30th, 2026, 7:30am"
    );
    for timezone in ["America/Chicago", "CST6CDT,M3.2.0,M11.1.0"] {
        for (instant, expected) in [
            ("2026-01-01T18:00:00Z", "Jan. 1st, 2026, 12pm"),
            ("2026-07-01T18:05:00Z", "July 1st, 2026, 1:05pm"),
            ("2026-03-08T07:59:00Z", "Mar. 8th, 2026, 1:59am"),
            ("2026-03-08T08:00:00Z", "Mar. 8th, 2026, 3am"),
            ("2026-11-01T06:59:00Z", "Nov. 1st, 2026, 1:59am"),
            ("2026-11-01T07:00:00Z", "Nov. 1st, 2026, 1am"),
        ] {
            assert_eq!(at(timezone, instant), expected);
        }
    }
    assert_eq!(
        at("+05:45", "2026-10-30T02:00:00Z"),
        "Oct. 30th, 2026, 7:45am"
    );
}

#[test]
fn ordinal_days_and_midnight_use_readable_english() {
    for (day, ordinal) in [
        (1, "1st"),
        (2, "2nd"),
        (3, "3rd"),
        (4, "4th"),
        (11, "11th"),
        (12, "12th"),
        (13, "13th"),
        (21, "21st"),
        (22, "22nd"),
        (23, "23rd"),
        (31, "31st"),
    ] {
        assert_eq!(
            at("UTC", &format!("2026-10-{day:02}T00:00:00Z")),
            format!("Oct. {ordinal}, 2026, 12am")
        );
    }
}

#[test]
fn unavailable_dates_never_fall_back_to_server_local_time() {
    for zone in [None, Some("invalid/timezone"), Some("tzif:invalid")] {
        assert_eq!(
            ClientTimeZone::from_client(zone).timestamp(Some(0)),
            "Time unavailable"
        );
    }
    let zone = ClientTimeZone::from_client(Some("UTC"));
    for ms in [None, Some(i64::MAX), Some(i64::MIN)] {
        assert_eq!(zone.timestamp(ms), "Time unavailable");
    }
    assert_eq!(zone.timestamp(Some(-1)), "Dec. 31st, 1969, 11:59pm");
}

fn anonymous_rules() -> Vec<u8> {
    // A minimal TZif v1 with a transition at the Unix epoch, from +02:00 to +00:30.
    let mut bytes = b"TZif\0".to_vec();
    bytes.extend([0; 15]);
    for count in [0u32, 0, 0, 1, 2, 8] {
        bytes.extend(count.to_be_bytes());
    }
    bytes.extend(0i32.to_be_bytes());
    bytes.push(1);
    for (seconds, abbreviation) in [(7200i32, 0), (1800, 4)] {
        bytes.extend(seconds.to_be_bytes());
        bytes.extend([0, abbreviation]);
    }
    bytes.extend(b"OLD\0NEW\0");
    bytes
}

#[test]
fn anonymous_client_timezone_preserves_historical_transitions() {
    let wire = format!("tzif:{}", STANDARD.encode(anonymous_rules()));
    let zone = ClientTimeZone::from_client(Some(&wire));
    assert_eq!(zone.timestamp(Some(-1)), "Jan. 1st, 1970, 1:59am");
    assert_eq!(zone.timestamp(Some(0)), "Jan. 1st, 1970, 12:30am");
}

#[test]
fn capture_uses_client_tz_override() {
    const CHILD: &str = "YINZ_TEST_CLIENT_TIMEZONE";
    if let Ok(timezone) = std::env::var(CHILD) {
        let captured = capture_timezone().unwrap();
        if std::env::var_os("YINZ_TEST_ANONYMOUS_TIMEZONE").is_some() {
            assert!(captured.starts_with("tzif:"), "{captured}");
        }
        let zone = ClientTimeZone::from_client(Some(&captured));
        let ms = "2026-10-30T02:00:00Z"
            .parse::<Timestamp>()
            .unwrap()
            .as_millisecond();
        assert_eq!(
            zone.timestamp(Some(ms)),
            at(&timezone, "2026-10-30T02:00:00Z")
        );
        return;
    }
    // Isolate TZ overrides and the system timezone cache from concurrent tests.
    for timezone in ["America/Chicago", "Asia/Tokyo", "CST6CDT,M3.2.0,M11.1.0"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "client_time::tests::capture_uses_client_tz_override",
                "--nocapture",
            ])
            .env(CHILD, timezone)
            .env("TZ", timezone)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("localtime");
    std::fs::write(&path, anonymous_rules()).unwrap();
    for prefix in ["", ":"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "client_time::tests::capture_uses_client_tz_override",
                "--nocapture",
            ])
            .env(CHILD, "+00:30")
            .env("YINZ_TEST_ANONYMOUS_TIMEZONE", "1")
            .env("TZ", format!("{prefix}{}", path.display()))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

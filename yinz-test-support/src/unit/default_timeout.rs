use super::*;

/// The `slow-timeout` line of `[profile.<name>]` in the workspace's nextest config, as the time
/// after which nextest kills a test: `period` times `terminate-after`.
fn nextest_kill_time(profile: &str) -> Duration {
    let config = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../.config/nextest.toml"
    ))
    .expect("read the workspace's nextest config");
    let header = format!("[profile.{profile}]");
    let slow_timeout = config
        .split(header.as_str())
        .nth(1)
        .and_then(|rest| {
            rest.lines()
                .take_while(|line| !line.starts_with('['))
                .find(|line| line.starts_with("slow-timeout"))
        })
        .unwrap_or_else(|| panic!("nextest profile {profile} has a slow-timeout"));
    let number_after = |key: &str| -> u64 {
        let rest = slow_timeout
            .split(key)
            .nth(1)
            .unwrap_or_else(|| panic!("{profile}'s slow-timeout has a {key}"));
        rest.chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or_else(|_| panic!("{profile}'s {key} is a whole number"))
    };
    Duration::from_secs(number_after("period") * number_after("terminate-after"))
}

/// `cargo x test` runs the `unit` profile and `cargo x integration-test` the `default` one; both
/// wait with this default. A failing wait has to panic with its diagnostic before the kill, with
/// room for whatever the test did before it started waiting.
#[test]
fn the_default_wait_ends_well_before_nextest_kills_the_test() {
    for profile in ["default", "unit"] {
        let killed_at = nextest_kill_time(profile);
        assert!(
            DEFAULT_WAIT + Duration::from_secs(10) <= killed_at,
            "default wait {DEFAULT_WAIT:?} must end well before nextest's {profile} profile kills \
             the test at {killed_at:?}"
        );
    }
    assert_eq!(timeout_from(None), DEFAULT_WAIT);
}

#[test]
fn the_wait_variable_sets_the_timeout() {
    assert_eq!(
        timeout_from(Some("10000".into())),
        Duration::from_millis(10_000)
    );
}

#[test]
#[should_panic(expected = "YINZ_TEST_WAIT_TIMEOUT_MS must be a whole number of milliseconds")]
fn a_malformed_wait_variable_is_refused() {
    timeout_from(Some("ten seconds".into()));
}

use super::*;

#[test]
fn the_shared_wait_variable_sets_harness_waits() {
    assert_eq!(
        timeout_from(Some("10000".into()), None),
        Duration::from_millis(10_000)
    );
}

#[test]
fn the_shared_variable_wins_over_the_legacy_one() {
    assert_eq!(
        timeout_from(Some("250".into()), Some("90000".into())),
        Duration::from_millis(250)
    );
    assert_eq!(
        timeout_from(None, Some("750".into())),
        Duration::from_millis(750)
    );
}

#[test]
fn the_default_wait_ends_before_nextest_terminates_the_test() {
    let config = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../.config/nextest.toml"
    ))
    .unwrap();
    let default_profile = config
        .split("[profile.default]")
        .nth(1)
        .and_then(|rest| rest.lines().find(|line| line.starts_with("slow-timeout")))
        .expect("default nextest profile has a slow-timeout");
    let number_after = |key: &str| -> u64 {
        let rest = default_profile.split(key).nth(1).unwrap();
        rest.chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap()
    };
    let terminated_after =
        Duration::from_secs(number_after("period") * number_after("terminate-after"));
    assert!(
        timeout_from(None, None) < terminated_after,
        "default wait {:?} must end before nextest kills the test at {terminated_after:?}",
        timeout_from(None, None)
    );
}

#[test]
#[should_panic(expected = "YINZ_TEST_WAIT_TIMEOUT_MS must be a whole number of milliseconds")]
fn a_malformed_wait_variable_is_refused() {
    timeout_from(Some("ten seconds".into()), None);
}

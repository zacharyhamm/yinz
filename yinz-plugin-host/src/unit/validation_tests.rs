use super::execution::timeout_duration;
use super::sessions::delete_dead_session;
use std::time::Duration;
use yinz_types::internal_ids::SessionName;

#[test]
fn dead_session_deletion_rejects_paths() {
    for invalid in ["", ".", "..", "../outside", "/tmp/outside"] {
        assert!(
            delete_dead_session(&SessionName::new(invalid)).is_err(),
            "{invalid:?}"
        );
    }
}

#[test]
fn invalid_plugin_timeouts_are_rejected() {
    for invalid in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
        assert!(
            timeout_duration(yinz_types::units::FractionalSeconds::new(invalid)).is_err(),
            "{invalid}"
        );
    }
    assert_eq!(
        timeout_duration(yinz_types::units::FractionalSeconds::new(0.5)).unwrap(),
        Duration::from_millis(500)
    );
}

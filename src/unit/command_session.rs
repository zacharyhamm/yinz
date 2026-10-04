use super::command_session_ready;
use std::{
    io::ErrorKind,
    time::{Duration, Instant},
};
use yinz_cli::sessions::{
    denied_session_message, session_listing_error_message, unresponsive_session_message,
};
use yinz_ipc::probe::SessionProbe;

#[test]
fn readiness_retries_only_unresponsive_sessions_and_is_bounded() {
    let session = "named".into();
    for (status, expected) in [
        (Ok(SessionProbe::Alive), Ok(())),
        (
            Ok(SessionProbe::Absent),
            Err("Session 'named' not found.".into()),
        ),
        (
            Ok(SessionProbe::Denied),
            Err(denied_session_message("named")),
        ),
        (
            Err(ErrorKind::PermissionDenied),
            Err(session_listing_error_message(ErrorKind::PermissionDenied)),
        ),
        (
            Err(ErrorKind::Other),
            Err(session_listing_error_message(ErrorKind::Other)),
        ),
    ] {
        let mut probes = 0;
        assert_eq!(
            command_session_ready(&session, Duration::from_secs(30), |name| {
                assert_eq!(name, &session);
                probes += 1;
                status
            }),
            expected
        );
        assert_eq!(probes, 1);
    }
    let start = Instant::now();
    let budget = Duration::from_millis(150);
    let mut probes = 0;
    assert_eq!(
        command_session_ready(&session, budget, |_| {
            probes += 1;
            Ok(SessionProbe::Unresponsive)
        }),
        Err(unresponsive_session_message("named"))
    );
    assert!(start.elapsed() >= budget);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(probes <= 2);
    assert!(
        command_session_ready(&"../invalid".into(), budget, |_| panic!(
            "invalid name was probed"
        ))
        .is_err()
    );
}

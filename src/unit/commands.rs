use super::{kill_outcome, named_session_exists_or_error, sessions_to_delete, sessions_to_kill};
use std::{cell::Cell, io, time::Duration};
use {
    yinz_cli::sessions::session_listing_error_message,
    yinz_cli::sessions::unresponsive_session_message, yinz_ipc::probe::SessionProbe,
};

fn session(name: &str) -> (yinz_types::internal_ids::SessionName, Duration) {
    (name.into(), Duration::default())
}

#[test]
fn session_selection_requires_one_active_session() {
    use yinz_cli::sessions::ActiveSession;

    assert_eq!(
        super::resolve_session(ActiveSession::One("work".into())).unwrap(),
        "work"
    );
    for active in [ActiveSession::None, ActiveSession::Many] {
        assert_eq!(
            super::resolve_session(active).unwrap_err().to_string(),
            "no session selected; pass --session SESSION"
        );
    }
}

#[test]
fn session_deletion_fails_when_live_session_discovery_fails() {
    let result = sessions_to_delete(false, vec![session("dead")], || {
        Err(io::ErrorKind::PermissionDenied)
    });

    assert_eq!(result.unwrap_err(), io::ErrorKind::PermissionDenied);
}

#[test]
fn session_deletion_excludes_live_sessions() {
    let sessions = sessions_to_delete(false, vec![session("live"), session("dead")], || {
        Ok(vec![session("live")])
    })
    .unwrap();

    assert_eq!(sessions, vec![session("dead")]);
}

#[test]
fn forced_session_deletion_skips_live_session_discovery() {
    let discovery_called = Cell::new(false);
    let eligible = vec![session("live"), session("dead")];
    let sessions = sessions_to_delete(true, eligible.clone(), || {
        discovery_called.set(true);
        Ok(vec![session("live")])
    })
    .unwrap();

    assert_eq!(sessions, eligible);
    assert!(!discovery_called.get());
}

#[test]
fn session_killing_includes_every_non_absent_socket() {
    let sessions = sessions_to_kill(|| {
        Ok(vec![
            (
                "alive".to_owned().into(),
                Duration::default(),
                SessionProbe::Alive,
            ),
            (
                "wedged".to_owned().into(),
                Duration::default(),
                SessionProbe::Unresponsive,
            ),
            (
                "absent".to_owned().into(),
                Duration::default(),
                SessionProbe::Absent,
            ),
        ])
    })
    .unwrap();

    assert_eq!(sessions, vec![session("alive"), session("wedged")]);
}

#[test]
fn a_kill_requires_completion_and_always_cleans_up_unconfirmed_shutdowns() {
    use std::cell::Cell;
    for probe in [SessionProbe::Alive, SessionProbe::Unresponsive] {
        assert!(kill_outcome(Ok(probe), || Ok(()), || panic!("already complete")).is_ok());
        for kind in [
            io::ErrorKind::TimedOut,
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::ConnectionRefused,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::InvalidData,
            io::ErrorKind::Other,
        ] {
            let swept = Cell::new(false);
            let result = kill_outcome(
                Ok(probe),
                || Err(kind.into()),
                || {
                    swept.set(true);
                    Ok(1)
                },
            );
            assert!(swept.get());
            assert!(result.is_err(), "fallback must not confirm server shutdown");
        }
    }
    assert!(kill_outcome(Ok(SessionProbe::Absent), || panic!("absent"), || Ok(1)).is_ok());
    assert!(kill_outcome(Ok(SessionProbe::Absent), || panic!("absent"), || Ok(0)).is_err());
    for probe in [
        Ok(SessionProbe::Absent),
        Err(io::ErrorKind::PermissionDenied),
    ] {
        let result = kill_outcome(
            probe,
            || panic!("not deliverable"),
            || Err("cleanup failed".into()),
        );
        assert!(result.unwrap_err().contains("cleanup failed"));
    }
}

#[test]
fn only_an_absent_named_session_allows_creation() {
    let creation_allowed = |probe| {
        named_session_exists_or_error(&yinz_types::internal_ids::SessionName::new("named"), probe)
            .map(|session_exists| !session_exists)
    };

    assert_eq!(creation_allowed(Ok(SessionProbe::Alive)), Ok(false));
    assert_eq!(creation_allowed(Ok(SessionProbe::Absent)), Ok(true));
    assert_eq!(
        creation_allowed(Ok(SessionProbe::Unresponsive)),
        Err(unresponsive_session_message("named"))
    );
    assert_eq!(
        creation_allowed(Err(io::ErrorKind::PermissionDenied)),
        Err(session_listing_error_message(
            io::ErrorKind::PermissionDenied
        ))
    );
}

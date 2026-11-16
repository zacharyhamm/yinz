use super::*;

fn fixture() -> (tempfile::TempDir, SessionInfo) {
    let temp = tempfile::tempdir().unwrap();
    let verifier = password::tests::ready(temp.path());
    let dir = temp.path().join("web");
    let _lock = password::lock(&dir).unwrap();
    let (_, session) = sessions::Store::default()
        .issue(
            temp.path(),
            &verifier.verifier_id,
            "phone".into(),
            true,
            30,
            SystemClock.now_ms(),
        )
        .unwrap();
    (temp, session)
}
fn subscription() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "endpoint": "https://web.push.apple.com/test",
        "expirationTime": null,
        "keys": { "p256dh": "BGa4N1PI79lboMR_YrwCiCsgp35DRvedt7opHcf0yM3iOBTSoQYqQLwWxAfRKE6tsDnReWmhsImkhDF_DBdkNSU", "auth": URL_SAFE_NO_PAD.encode([5; 16]) }
    })).unwrap()
}
#[test]
fn signing_key_survives_restarts_and_subscriptions_follow_signin_revocation() {
    let (data, session) = fixture();
    let first = api(data.path(), &session, None).unwrap();
    assert_eq!(
        URL_SAFE_NO_PAD
            .decode(first["public_key"].as_str().unwrap())
            .unwrap()
            .len(),
        65
    );
    api(data.path(), &session, Some(&subscription())).unwrap();
    assert_eq!(api(data.path(), &session, None).unwrap(), first);
    assert_eq!(targets(data.path()).unwrap().1.len(), 1);
    let auth = crate::auth::Auth::new(
        data.path().into(),
        Default::default(),
        std::sync::Arc::new(SystemClock),
    );
    assert_eq!(auth.logout(&session).status, 204);
    assert!(targets(data.path()).unwrap().1.is_empty());
    assert!(api(data.path(), &session, Some(&subscription())).is_err());
}
#[test]
fn rotation_disable_and_invalid_subscription_fail_closed() {
    let (data, session) = fixture();
    for endpoint in [
        "http://web.push.apple.com/test",
        "https://localhost/test",
        "https://web.push.apple.com.evil.test/test",
        "https://web.push.apple.com:8443/test",
        "https://user@web.push.apple.com/test",
        "https://127.0.0.1/test",
    ] {
        assert!(!valid_endpoint(endpoint), "{endpoint}");
    }
    assert!(valid_endpoint("https://fcm.googleapis.com/fcm/send/test"));
    assert!(valid_endpoint(
        "https://updates.push.services.mozilla.com/wpush/test"
    ));
    api(data.path(), &session, Some(&subscription())).unwrap();
    let disable = br#"{"endpoint":"https://web.push.apple.com/test"}"#;
    api(data.path(), &session, Some(disable)).unwrap();
    assert!(targets(data.path()).unwrap().1.is_empty());
    api(data.path(), &session, Some(&subscription())).unwrap();
    password::revoke_sessions(data.path()).unwrap();
    assert!(targets(data.path()).unwrap().1.is_empty());
}
#[test]
fn private_store_rejects_symlinks() {
    let (data, session) = fixture();
    api(data.path(), &session, Some(&subscription())).unwrap();
    let path = data.path().join("web/push.json");
    let original = data.path().join("original.json");
    std::fs::rename(&path, &original).unwrap();
    std::os::unix::fs::symlink(&original, &path).unwrap();
    assert!(api(data.path(), &session, None).is_err());
    assert!(targets(data.path()).is_err());
}

#[test]
fn independent_session_writers_preserve_other_devices_and_stale_cleanup_preserves_replacements() {
    let (data, session) = fixture();
    let one = data.path().to_owned();
    let two = one.clone();
    let other = session.clone();
    let first = std::thread::spawn(move || api(&one, &other, Some(&subscription())).unwrap());
    let second = std::thread::spawn(move || {
        let mut body: serde_json::Value = serde_json::from_slice(&subscription()).unwrap();
        body["endpoint"] = "https://fcm.googleapis.com/test".into();
        api(&two, &session, Some(&serde_json::to_vec(&body).unwrap())).unwrap()
    });
    assert_eq!(first.join().unwrap(), second.join().unwrap());
    let (key, rows) = targets(data.path()).unwrap();
    assert_eq!(rows.len(), 2);
    let request = web_push::request_builder::build_request::<Vec<u8>>(
        message(&key, &rows[0], "secret body").unwrap(),
    );
    assert_eq!(request.headers()["content-encoding"], "aes128gcm");
    assert!(request.headers()["authorization"]
        .to_str()
        .unwrap()
        .starts_with("vapid "));
    assert_eq!(request.headers()["ttl"], "3600");
    assert!(!request
        .body()
        .windows(11)
        .any(|bytes| bytes == b"secret body"));
    let mut stale = rows[0].clone();
    stale.auth = URL_SAFE_NO_PAD.encode([9; 16]);
    remove(data.path(), &stale).unwrap();
    assert_eq!(targets(data.path()).unwrap().1.len(), 2);
    remove(data.path(), &rows[0]).unwrap();
    assert_eq!(targets(data.path()).unwrap().1.len(), 1);
}

#[test]
fn disabling_revokes_queued_targets_and_json_payloads_fit_encrypted_records() {
    let (data, session) = fixture();
    api(data.path(), &session, Some(&subscription())).unwrap();
    let (key, rows) = targets(data.path()).unwrap();
    assert!(authorized(data.path(), &rows[0]).unwrap());
    api(
        data.path(),
        &session,
        Some(br#"{"endpoint":"https://web.push.apple.com/test"}"#),
    )
    .unwrap();
    assert!(!authorized(data.path(), &rows[0]).unwrap());
    use yinz_types::{
        internal_ids::NotificationId,
        notifications::{
            NotificationData, NotificationDecision, NotificationDelivery, NotificationSource,
        },
        units::UnixMillis,
    };
    let notification = NotificationResolution::from_decision(
        NotificationData {
            id: NotificationId::new(1),
            received_at_unix_ms: UnixMillis::new(1),
            source: NotificationSource::Toast,
            pane_id: None,
            plugin_id: None,
            title: "\u{0001}".repeat(1000),
            body: "\u{0002}".repeat(10000),
            delivery: NotificationDelivery::Toast,
        },
        NotificationDecision::Pass,
        None,
    );
    let text = payload(&"\u{0003}".repeat(1000), &notification);
    assert!(text.len() <= 3000);
    let message = message(&key, &rows[0], &text).unwrap();
    assert!(message.payload.unwrap().content.len() <= 4096);
}

#[test]
fn resumed_signin_survives_old_idle_deadline_and_other_store_writes() {
    let data = tempfile::tempdir().unwrap();
    let v = password::tests::ready(data.path()).verifier_id;
    let mut store = sessions::Store::default();
    let deadline = SystemClock.now_ms() + 60_000;
    let (token, session) = {
        let _lock = password::lock(&data.path().join("web")).unwrap();
        store
            .issue(
                data.path(),
                &v,
                "phone".into(),
                false,
                30,
                deadline - 12 * 3_600_000,
            )
            .unwrap()
    };
    api(data.path(), &session, Some(&subscription())).unwrap();
    {
        // This write formerly reset the shared five-minute flush timer, leaving
        // the phone's next touch buffered past its persisted idle deadline.
        let _lock = password::lock(&data.path().join("web")).unwrap();
        store
            .issue(
                data.path(),
                &v,
                "other".into(),
                false,
                30,
                deadline - 120_000,
            )
            .unwrap();
    }
    let resumed = store
        .authenticate(data.path(), &v, &token, deadline - 60_000)
        .unwrap()
        .unwrap();
    let later = deadline + 60_000;
    assert!(active(data.path(), &v, &session.id, later).unwrap());
    assert_eq!(targets_at(data.path(), later).unwrap().1.len(), 1);
    // An independent writer must also see the renewal when it prunes sessions.
    {
        let _lock = password::lock(&data.path().join("web")).unwrap();
        sessions::Store::default()
            .issue(data.path(), &v, "third".into(), false, 30, later)
            .unwrap();
    }
    assert!(active(data.path(), &v, &session.id, later).unwrap());
    assert_eq!(targets_at(data.path(), later).unwrap().1.len(), 1);
    assert_eq!(read(data.path()).unwrap().subscriptions.len(), 1);
    // No grace period: actual idle expiry still removes the subscription.
    assert!(!active(data.path(), &v, &session.id, resumed.idle_expires_at_ms).unwrap());
    assert!(targets_at(data.path(), resumed.idle_expires_at_ms)
        .unwrap()
        .1
        .is_empty());
    assert!(read(data.path()).unwrap().subscriptions.is_empty());
}

#[test]
fn absolute_expiry_prunes_push_subscriptions() {
    let (data, session) = fixture();
    api(data.path(), &session, Some(&subscription())).unwrap();
    let v = verifier(data.path()).unwrap();
    // Keep idle expiry beyond the absolute deadline to isolate the hard cap.
    let mut snapshot = sessions::read(data.path(), &v).unwrap();
    snapshot.sessions[0].last_seen_ms = session.expires_at_ms - 1;
    password::atomic_write(
        &data.path().join("web"),
        "sessions.json",
        &serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
    assert!(active(data.path(), &v, &session.id, session.expires_at_ms - 1).unwrap());
    assert!(!active(data.path(), &v, &session.id, session.expires_at_ms).unwrap());
    assert!(targets_at(data.path(), session.expires_at_ms)
        .unwrap()
        .1
        .is_empty());
    assert!(read(data.path()).unwrap().subscriptions.is_empty());
}

use super::*;
use crate::auth::{AuthConfig, AuthError, Clock};
use std::sync::atomic::AtomicI64;
struct Time(AtomicI64);
impl Clock for Time {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::Acquire)
    }
}
fn fixture() -> (tempfile::TempDir, Arc<Auth>) {
    let t = tempfile::tempdir().unwrap();
    password::tests::ready(t.path());
    let a = Arc::new(Auth::new(
        t.path().into(),
        AuthConfig::default(),
        Arc::new(Time(AtomicI64::new(1000))),
    ));
    (t, a)
}
async fn login(a: &Arc<Auth>, body: &str) -> AuthResponse {
    a.login(
        Zeroizing::new(body.as_bytes().to_vec()),
        Some("Browser"),
        "client",
    )
    .await
}
const GOOD: &str = r#"{"password":"twelve words long","label":"  My   phone  ","remember":true}"#;
#[tokio::test]
async fn valid_login_session_logout_rotation_independent_state_and_secret_debug() {
    let (t, a) = fixture();
    let r = login(&a, GOOD).await;
    assert_eq!(r.status, 200);
    let token = r.json["token"].as_str().unwrap();
    assert!(!format!("{r:?}").contains(token));
    let info = a.authenticate(Some(token)).unwrap();
    assert_eq!(info.label, "My phone");
    assert_eq!(a.session_json(&info).json["session"]["id"], info.id);
    let b = Auth::new(
        t.path().into(),
        AuthConfig::default(),
        Arc::new(Time(AtomicI64::new(1000))),
    );
    assert!(b.authenticate(Some(token)).is_ok());
    assert_eq!(b.logout(&info).status, 204);
    assert_eq!(a.authenticate(Some(token)), Err(AuthError::Unauthenticated));
    let r = login(&a, GOOD).await;
    let token = r.json["token"].as_str().unwrap();
    assert!(b.authenticate(Some(token)).is_ok());
    assert_eq!(a.logout_all().status, 204);
    assert_eq!(b.authenticate(Some(token)), Err(AuthError::Unauthenticated));
    password::clear(t.path()).unwrap();
    assert_eq!(
        a.authenticate(Some(token)),
        Err(AuthError::PasswordNotReady)
    );
}
#[tokio::test]
async fn strict_dto_and_store_errors_are_sanitized() {
    let (t, a) = fixture();
    for body in [
        "{}",
        "{",
        r#"{"password":3}"#,
        r#"{"password":"a","password":"b"}"#,
        r#"{"password":"a","extra":true}"#,
        r#"{"password":"a","label":null}"#,
        r#"{"password":"a","remember":null}"#,
        r#"{"password":"a\u0000"}"#,
        r#"{"password":"a","label":"\u0001"}"#,
    ] {
        assert_eq!(login(&a, body).await.status, 400, "{body}");
    }
    assert_eq!(login(&a, r#"{"password":"wrong"}"#).await.status, 401);
    let r = login(&a, &format!(r#"{{"password":"{}"}}"#, "x".repeat(1025))).await;
    assert_eq!(r.status, 400);
    password::atomic_write(
        &t.path().join("web"),
        "sessions.json",
        b"private corrupt contents",
    )
    .unwrap();
    let r = login(&a, GOOD).await;
    assert_eq!(r.status, 503);
    assert_eq!(r.json["error"]["code"], "session_store_unavailable");
    assert!(!r.json.to_string().contains("private corrupt"));
}
// Use an explicit gate rather than the speed of Argon2 to control cancellation and queueing.
fn gated(
    a: &Arc<Auth>,
) -> (
    std::sync::mpsc::Receiver<()>,
    Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let g = gate.clone();
    *a.verify_hook.lock().unwrap() = Some(Arc::new(move || {
        tx.send(()).unwrap();
        let (m, c) = &*g;
        let mut open = m.lock().unwrap();
        while !*open {
            open = c.wait(open).unwrap();
        }
    }));
    (rx, gate)
}
async fn entered(rx: std::sync::mpsc::Receiver<()>) -> std::sync::mpsc::Receiver<()> {
    tokio::task::spawn_blocking(move || {
        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        rx
    })
    .await
    .unwrap()
}
fn open(g: &Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>) {
    *g.0.lock().unwrap() = true;
    g.1.notify_all();
}
#[tokio::test]
async fn bounded_queue_cancellation_keeps_running_permits_and_runtime_responsive() {
    let (_t, a) = fixture();
    let (rx, g) = gated(&a);
    let auth = a.clone();
    let active = tokio::spawn(async move { login(&auth, GOOD).await });
    let rx = entered(rx).await;
    active.abort();
    assert!(active.await.unwrap_err().is_cancelled());
    assert_eq!(a.admission.available_permits(), 2);
    assert_eq!(a.verification.available_permits(), 0);
    let mut waiters = vec![];
    for _ in 0..2 {
        let auth = a.clone();
        waiters.push(tokio::spawn(async move { login(&auth, GOOD).await }));
    }
    // Poll the futures explicitly so queue admission, not scheduler timing, is the predicate.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while a.admission.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let r = login(&a, GOOD).await;
    assert_eq!(r.status, 429);
    assert_eq!(r.retry_after_s, Some(1));
    let canceled = waiters.pop().unwrap();
    canceled.abort();
    assert!(canceled.await.unwrap_err().is_cancelled());
    assert_eq!(a.admission.available_permits(), 1);
    open(&g);
    let r = waiters.pop().unwrap().await.unwrap();
    assert_eq!(r.status, 200);
    let _ = entered(rx).await;
    assert_eq!(a.admission.available_permits(), 3);
    assert_eq!(a.verification.available_permits(), 1);
}
#[tokio::test]
async fn rotation_during_hash_does_not_issue_old_verifier_and_canceled_work_cannot_issue() {
    let (t, a) = fixture();
    let (rx, g) = gated(&a);
    let auth = a.clone();
    let request = tokio::spawn(async move { login(&auth, GOOD).await });
    let _ = entered(rx).await;
    password::revoke_sessions(t.path()).unwrap();
    open(&g);
    assert_eq!(request.await.unwrap().status, 401);
    assert!(!t.path().join("web/sessions.json").exists());
    let (rx, g) = gated(&a);
    let auth = a.clone();
    let request = tokio::spawn(async move { login(&auth, GOOD).await });
    let _ = entered(rx).await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    open(&g);
    let permit = a.verification.clone().acquire_owned().await.unwrap();
    drop(permit);
    assert!(!t.path().join("web/sessions.json").exists());
}

#[tokio::test]
async fn canceled_commit_waiter_and_persistence_failure_never_report_success() {
    let (t, a) = fixture();
    let (tx, rx) = std::sync::mpsc::channel();
    *a.commit_hook.lock().unwrap() = Some(Arc::new(move || {
        tx.send(()).unwrap();
    }));
    let lock = password::lock(&t.path().join("web")).unwrap();
    let auth = a.clone();
    let task = tokio::spawn(async move { login(&auth, GOOD).await });
    let _ = entered(rx).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(lock);
    let permit = a.verification.clone().acquire_owned().await.unwrap();
    drop(permit);
    assert!(!t.path().join("web/sessions.json").exists());
    *a.commit_hook.lock().unwrap() = None;
    let response = login(&a, GOOD).await;
    let token = response.json["token"].as_str().unwrap();
    let info = a.authenticate(Some(token)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let web = t.path().join("web");
    std::fs::set_permissions(&web, std::fs::Permissions::from_mode(0o500)).unwrap();
    let failed = a.logout(&info);
    std::fs::set_permissions(&web, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(failed.status, 503);
    assert!(a.authenticate(Some(token)).is_ok());

    std::fs::set_permissions(
        t.path().join("web/sessions.json"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert_eq!(a.logout(&info).status, 503);
    assert_eq!(
        a.authenticate(Some(token)),
        Err(AuthError::StoreUnavailable)
    );
    let before = std::fs::read(t.path().join("web/sessions.json")).unwrap();
    assert_eq!(login(&a, GOOD).await.status, 503);
    assert_eq!(
        before,
        std::fs::read(t.path().join("web/sessions.json")).unwrap()
    );
}

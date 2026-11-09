use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};
use yinz_integration_tests::{claim_first_terminal_and_wait_for_prompt, TestRunner, TERMINAL_SIZE};
use yinz_test_support::{default_timeout, Watched};
use yinz_types::input::actions::Action;
use yinz_web::{auth::password, listener_file::ListenerRecord};
use zeroize::Zeroizing;

// Polling durable records is intentional: they are written by the server on its own runtime.
// Feed observations into Watched; every test waits on a predicate and joins its observer.
struct Records {
    values: Watched<Vec<ListenerRecord>>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    observer: Option<std::thread::JoinHandle<()>>,
}
impl Records {
    fn new(dir: PathBuf) -> Self {
        let values = Watched::new(Vec::new());
        let output = values.clone();
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let observer = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
                let mut tick = tokio::time::interval(Duration::from_millis(20));
                loop {
                    tokio::select! {
                        _ = &mut cancelled => break,
                        _ = tick.tick() => {
                            let records = std::fs::read_dir(dir.join("web/listeners")).into_iter().flatten()
                                .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
                                .filter_map(|b| serde_json::from_slice(&b).ok()).collect();
                            *output.lock() = records;
                        }
                    }
                }
            });
        });
        Self {
            values,
            cancel: Some(cancel),
            observer: Some(observer),
        }
    }
    fn serving(&self, session: &str) -> SocketAddr {
        self.values.wait_until(
            "session's web listener record",
            default_timeout(),
            |records| {
                records
                    .iter()
                    .find(|r| r.session == session)
                    .map(|r| r.address)
            },
        )
    }
    fn empty(&self) {
        self.values
            .wait_until("owned listener records removed", default_timeout(), |r| {
                r.is_empty().then_some(())
            });
    }
}
impl Drop for Records {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        self.observer.take().unwrap().join().unwrap();
    }
}
fn private_data() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), temp.path()).unwrap();
    temp
}
fn set(dir: &Path) {
    password::write(dir, &Zeroizing::new("integration password".into())).unwrap();
}
fn get(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

#[test]
fn enabled_health_starting_rename_clear_and_terminal_usable() {
    let dir = private_data();
    set(dir.path());
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; plugin \"file:/missing-web-app.scm\"; }")
        .start();
    let terminal = claim_first_terminal_and_wait_for_prompt(&session);
    let addr = records.serving(session.session_name());
    let reply = get(addr, "/healthz");
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.contains("\"ok\":true"));
    assert!(reply.contains("x-content-type-options: nosniff"));
    let reply = get(addr, "/");
    assert!(reply.starts_with("HTTP/1.1 503"));
    assert!(reply.contains("Yinz web is starting"));
    terminal.output(b"WEB TERMINAL STILL USABLE\r\n");
    session.wait_until("terminal usable without web app", |g| {
        g.contains("WEB TERMINAL STILL USABLE")
    });
    let renamed = format!("{}-renamed", session.session_name());
    assert_eq!(
        session
            .run_actions_reporting(vec![Action::RenameSession {
                name: renamed.clone().into()
            }])
            .0,
        0
    );
    assert_eq!(records.serving(&renamed), addr);
    assert!(get(addr, "/healthz").contains(&renamed));
    password::clear(dir.path()).unwrap();
    records.empty();
    assert!(TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err());
    session.quit();
}

#[test]
fn missing_invalid_password_then_write_binds_and_config_waits_for_next_start() {
    let dir = private_data();
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    claim_first_terminal_and_wait_for_prompt(&session);
    assert!(records.values.get().is_empty());
    assert_eq!(
        password::read_state(dir.path()),
        password::PasswordState::Missing
    );
    yinz_runtime::shared::ensure_secure_dir(&dir.path().join("web")).unwrap();
    std::fs::write(dir.path().join("web/password"), "invalid").unwrap();
    assert!(matches!(
        password::read_state(dir.path()),
        password::PasswordState::Invalid(_)
    ));
    assert!(records.values.get().is_empty());
    set(dir.path());
    let addr = records.serving(session.session_name());
    assert_eq!(
        session
            .run_actions_reporting(vec![Action::Reconfigure {
                config: "web { enabled false; port 1; }".into(),
                write_to_disk: false
            }])
            .0,
        0
    );
    assert!(get(addr, "/healthz").starts_with("HTTP/1.1 200"));
    session.quit();
    records.empty();
    let mut disabled = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .start();
    claim_first_terminal_and_wait_for_prompt(&disabled);
    assert!(records.values.get().is_empty());
    disabled.quit();
}

#[test]
fn two_sessions_share_password_and_take_over_port_after_shutdown() {
    let dir = private_data();
    set(dir.path());
    let records = Records::new(dir.path().to_owned());
    let mut first = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    claim_first_terminal_and_wait_for_prompt(&first);
    let addr = records.serving(first.session_name());
    let mut second = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config(&format!("web {{ enabled true; port {}; }}", addr.port()))
        .start();
    claim_first_terminal_and_wait_for_prompt(&second);
    assert!(get(addr, "/healthz").contains(first.session_name()));
    first.quit();
    assert_eq!(records.serving(second.session_name()), addr);
    assert!(get(addr, "/healthz").contains(second.session_name()));
    second.quit();
    records.empty();
}

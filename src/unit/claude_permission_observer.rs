//! Actual native startup/observer/reporter processes; the peer is deliberately fake.
use super::*;
use std::io::{BufRead, BufReader};
use yinz_types::agent::{claude_permissions::Status, HookEvent};

#[cfg(target_os = "linux")]
#[test]
fn claude_observer_permission_starts_without_transcript_and_retries_failed_release() {
    struct Fixture {
        provider: Option<Child>,
        peer: Option<std::thread::JoinHandle<()>>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(child) = &mut self.provider {
                let _ = child.kill();
                let _ = child.wait();
            }
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(peer) = self.peer.take() {
                peer.join().unwrap();
            }
            // All detached descendants see provider death and exit their bounded reporters.
            // WNOHANG also handles continued children on macOS in fixtures that use this pattern.
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match nix::sys::wait::waitpid(None, Some(nix::sys::wait::WaitPidFlag::WNOHANG)) {
                    Ok(nix::sys::wait::WaitStatus::StillAlive) => {
                        assert!(
                            Instant::now() < deadline,
                            "permission fixture child did not exit"
                        );
                        std::thread::park_timeout(Duration::from_millis(10));
                    },
                    Ok(_) | Err(nix::errno::Errno::EINTR) => {},
                    Err(nix::errno::Errno::ECHILD) => break,
                    error => panic!("reap permission fixture: {error:?}"),
                }
            }
            nix::sys::prctl::set_child_subreaper(false).unwrap();
        }
    }
    warm_binary();
    nix::sys::prctl::set_child_subreaper(true).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let listener = listener(dir.path());
    listener
        .set_nonblocking(interprocess::local_socket::ListenerNonblockingMode::Accept)
        .unwrap();
    let reports = yinz_test_support::Watched::new(Vec::<HookEvent>::new());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut fixture = Fixture {
        provider: None,
        peer: None,
        stop: stop.clone(),
    };
    let observed = reports.clone();
    fixture.peer = Some(std::thread::spawn(move || {
        let mut active = false;
        let mut rejected_release = false;
        while !stop.load(std::sync::atomic::Ordering::SeqCst) {
            let socket = match listener.accept() {
                Ok(socket) => socket,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::park_timeout(Duration::from_millis(5));
                    continue;
                },
                Err(e) => panic!("permission peer accept: {e}"),
            };
            let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(socket);
            receiver
                .set_recv_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let IpcRecv::Msg(ClientToServerMsg::AgentReport(report), _) =
                receiver.recv_client_msg()
            else {
                panic!("permission report missing");
            };
            let hook: HookEvent = serde_json::from_str(&report.event.unwrap().raw).unwrap();
            let status = hook.claude_permission.as_ref().map(|sample| sample.status);
            if active && status == Some(Status::Inactive) && !rejected_release {
                rejected_release = true;
                // Lose the acknowledgement before application. The observer must retry.
                drop(receiver);
                continue;
            }
            receiver
                .get_sender::<ServerToClientMsg>()
                .unwrap()
                .send_server_msg(ServerToClientMsg::AgentReportAccepted {
                    report_id: report.report_id,
                })
                .unwrap();
            active |= status == Some(Status::Active);
            observed.lock().push(hook);
        }
    }));
    let provider_path = dir.path().join("claude");
    yinz_test_support::write_warm_script(
        &provider_path,
        "\"$@\" < \"$YINZ_TEST_START\"\nprintf 'ready\\n'\nread -r gate",
    );
    let start = dir.path().join("start.json");
    std::fs::write(&start, serde_json::json!({"hook_event_name":"SessionStart","source":"startup","session_id":"conversation"}).to_string()).unwrap();
    let base = command(dir.path());
    let mut provider = Command::new(provider_path);
    provider
        .arg(base.get_program())
        .args(base.get_args())
        .arg("--hook-json");
    for (key, value) in base.get_envs() {
        if let Some(value) = value {
            provider.env(key, value);
        } else {
            provider.env_remove(key);
        }
    }
    fixture.provider = Some(
        provider
            .env("YINZ_TEST_START", start)
            .env("CLAUDE_CONFIG_DIR", dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut output = BufReader::new(fixture.provider.as_mut().unwrap().stdout.take().unwrap());
    let mut ready = String::new();
    output.read_line(&mut ready).unwrap();
    assert_eq!(ready.trim(), "ready");
    let source = reports.wait_until(
        "native startup source",
        Duration::from_secs(10),
        |reports| {
            reports
                .iter()
                .find_map(|hook| hook.claude_notifications.as_ref()?.source.clone())
        },
    );
    let registry = dir
        .path()
        .join("sessions")
        .join(format!("{}.json", source.pid));
    std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
    let updated = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let publish = |status: &str, reason: &str, at: u64| {
        let temp = registry.with_extension("next");
        std::fs::write(&temp,serde_json::json!({"pid":source.pid,"sessionId":"conversation","kind":"interactive","status":status,"waitingFor":reason,"statusUpdatedAt":at}).to_string()).unwrap();
        std::fs::rename(temp, &registry).unwrap();
    };
    publish("busy", "", updated);
    reports.wait_until(
        "observer running without a transcript",
        Duration::from_secs(10),
        |reports| {
            reports
                .iter()
                .any(|h| {
                    h.claude_permission
                        .as_ref()
                        .is_some_and(|s| s.status == Status::Inactive)
                })
                .then_some(())
        },
    );
    let began = Instant::now();
    publish("waiting", "permission prompt", updated + 1);
    reports.wait_until(
        "pre-notification permission report",
        Duration::from_secs(1),
        |reports| {
            reports
                .iter()
                .any(|h| {
                    h.claude_permission
                        .as_ref()
                        .is_some_and(|s| s.status == Status::Active)
                })
                .then_some(())
        },
    );
    assert!(began.elapsed() < Duration::from_secs(1));
    publish("busy", "", updated + 2);
    reports.wait_until(
        "release retried without another native hook",
        Duration::from_secs(5),
        |reports| {
            reports
                .iter()
                .any(|h| {
                    h.claude_permission.as_ref().is_some_and(|s| {
                        s.updated_ms == updated + 2 && s.status == Status::Inactive
                    })
                })
                .then_some(())
        },
    );
    assert!(reports
        .lock()
        .iter()
        .filter(|h| h.claude_permission.is_some())
        .all(|h| h.agent_id.is_none() && h.prompt_id.is_none()));
}

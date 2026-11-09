//! Real native reporter/observer and registry fixture through the fake-OS whole app.
//! No live Claude inference is used. The provider and three-second resolution are test-owned.
use super::*;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread::JoinHandle,
    time::Instant,
};
use yinz_test_support::Watched;

const PLUGIN: &str = r#"
(yinz-declare-pane)
(define ready #f) (define pw 0) (define ew 0) (define pr 0) (define er 0)
(define pane-waiting #f) (define event-waiting #f)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState"))
    (lambda (_) (yinz-command (hash "Subscribe" '("PaneUpdate" "AgentEvent")))
      (set! ready #t) (yinz-request-render))))
(define (update event)
  (when (hash? event)
    (when (hash-contains? event "PaneUpdate")
      (define groups (hash-ref (hash-ref event "PaneUpdate") "panes"))
      (for-each (lambda (key)
        (for-each (lambda (pane)
          (define agent (hash-ref pane "agent"))
          (when (and (hash? agent) (equal? (hash-ref agent "kind") "claude"))
            (define waiting (equal? (hash-ref agent "state") "Waiting"))
            (when (and waiting (not pane-waiting)) (set! pw (+ pw 1)))
            (when (and pane-waiting (not waiting)) (set! pr (+ pr 1)))
            (set! pane-waiting waiting))) (hash-ref groups key))) (hash-keys->list groups)))
    (when (hash-contains? event "AgentEvent")
      (define hook (hash-ref event "AgentEvent"))
      (when (equal? (hash-ref hook "hook") "ClaudePermissionStatus")
        (define waiting (equal? (hash-ref hook "state") "Waiting"))
        (when (and waiting (not event-waiting)) (set! ew (+ ew 1)))
        (when (and event-waiting (not waiting)) (set! er (+ er 1)))
        (set! event-waiting waiting))))
  #t)
(define (render rows columns)
  (if ready (string-append "PERMISSION PW=" (number->string pw) " EW=" (number->string ew)
      " PR=" (number->string pr) " ER=" (number->string er)) "Waiting for permissions"))
"#;

fn direct_children() -> std::collections::BTreeSet<i32> {
    let mut children = std::collections::BTreeSet::new();
    for task in fs::read_dir("/proc/self/task").unwrap() {
        if let Ok(text) = fs::read_to_string(task.unwrap().path().join("children")) {
            children.extend(
                text.split_whitespace()
                    .map(|pid| pid.parse::<i32>().unwrap()),
            );
        }
    }
    children
}

struct Processes {
    baseline: std::collections::BTreeSet<i32>,
    children: Vec<Child>,
    readers: Vec<JoinHandle<()>>,
}
impl Processes {
    fn new() -> Self {
        nix::sys::prctl::set_child_subreaper(true).unwrap();
        Self {
            baseline: direct_children(),
            children: vec![],
            readers: vec![],
        }
    }
}
impl Drop for Processes {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            // Existing supervised Scheme workers belong to the server and must be reaped
            // by their owners. Only this fixture launches processes after the plugin-ready
            // barrier. Its short-lived native reporters orphan their detached observers here.
            let children = direct_children()
                .difference(&self.baseline)
                .copied()
                .collect::<Vec<_>>();
            if children.is_empty() {
                break;
            }
            for pid in children {
                match nix::sys::wait::waitpid(
                    nix::unistd::Pid::from_raw(pid),
                    Some(nix::sys::wait::WaitPidFlag::WNOHANG),
                ) {
                    Ok(_) | Err(nix::errno::Errno::EINTR | nix::errno::Errno::ECHILD) => {},
                    error => panic!("reap permission fixture: {error:?}"),
                }
            }
            assert!(
                Instant::now() < deadline,
                "permission fixture descendants did not exit"
            );
            std::thread::park_timeout(Duration::from_millis(10));
        }
        nix::sys::prctl::set_child_subreaper(false).unwrap();
    }
}

fn cli(binary: &Path, session: &str, config: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .args(["--session", session, "--config-dir"])
        .arg(config)
        .env_remove("YINZ_PANE_REF")
        .env_remove("YINZ_PANE_ID")
        .env_remove("YINZ_REMOTE_PANE_TOKEN")
        .env_remove("YINZ_REMOTE_CONTROL_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn registry(path: &Path, pid: u32, status: &str, reason: &str, updated: u64) {
    let temporary = path.with_extension("next");
    fs::write(
        &temporary,
        serde_json::json!({"pid":pid,"sessionId":"permission-session","kind":"interactive",
        "status":status,"waitingFor":reason,"statusUpdatedAt":updated})
        .to_string(),
    )
    .unwrap();
    fs::rename(temporary, path).unwrap();
}

/// This independent clock is the behavior under test, not synchronization by sleeping.
struct Resolution {
    cancel: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
    done: Watched<bool>,
}
impl Resolution {
    fn after(path: PathBuf, pid: u32, at: u64, duration: Duration) -> Self {
        let (cancel, cancelled) = mpsc::channel();
        let done = Watched::new(false);
        let published = done.clone();
        let thread = std::thread::spawn(move || {
            if matches!(
                cancelled.recv_timeout(duration),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                registry(&path, pid, "busy", "", at);
                *published.lock() = true;
            }
        });
        Self {
            cancel,
            thread: Some(thread),
            done,
        }
    }
}
impl Drop for Resolution {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        self.thread.take().unwrap().join().unwrap();
    }
}

#[test]
fn claude_permission_latency_parent_and_background_worker_reach_all_consumers() {
    let binary = PathBuf::from(
        std::env::var_os("YINZ_COMPILATION_WORKER").expect("run via cargo x integration-test"),
    );
    assert!(Command::new(&binary)
        .arg("--version")
        .status()
        .unwrap()
        .success());
    let dir = tempfile::tempdir().unwrap();
    let plugin = dir.path().join("permission-observer.scm");
    fs::write(&plugin, PLUGIN).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(dir.path().to_owned())
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ pane focus=true {{ plugin location=\"file:{}\"; }}; pane command=\"claude\"; }}",plugin.display()
        ))).start();
    let agent = yinz.expect_pty_spawn();
    let pane = PaneId::Terminal(agent.terminal_id());
    yinz.wait_until("permission observer plugin asks for access", |s| {
        s.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("plugin subscribed before any registry publication", |s| {
        s.contains("PERMISSION PW=0 EW=0 PR=0 ER=0")
    });
    let (cols, _) = agent.wait_for_size("Claude fixture gets its dimensions", |cols, rows| {
        cols > 20 && rows > 8
    });
    let border = "─".repeat(usize::from(cols));
    agent.output(format!("\x1b[?2004h\x1b[2J\x1b[HClaude permission fixture\r\n{border}\r\n❯ \r\n{border}\r\n  manual mode\x1b[3;3H\x1b[?25h").as_bytes());
    yinz.wait_until("Claude composer is visible", |s| {
        s.contains("Claude permission fixture")
    });

    let mut processes = Processes::new();
    let transcript = dir.path().join("root.jsonl");
    let row = serde_json::json!({"type":"user","sessionId":"permission-session","promptId":"parent","uuid":"user"});
    fs::write(&transcript, format!("{row}\n")).unwrap();
    for (name, hook) in [
        (
            "start",
            serde_json::json!({"hook_event_name":"SessionStart","session_id":"permission-session","source":"startup"}),
        ),
        (
            "submit",
            serde_json::json!({"hook_event_name":"UserPromptSubmit","session_id":"permission-session","prompt_id":"parent","prompt":"review","transcript_path":transcript}),
        ),
        (
            "parent-request",
            serde_json::json!({"hook_event_name":"PermissionRequest","session_id":"permission-session","prompt_id":"parent","tool_name":"Write","permission_mode":"default"}),
        ),
        (
            "worker-request",
            serde_json::json!({"hook_event_name":"PermissionRequest","session_id":"permission-session","agent_id":"background-worker","tool_name":"Bash","permission_mode":"default"}),
        ),
        (
            "late",
            serde_json::json!({"hook_event_name":"Notification","session_id":"permission-session","notification_type":"permission_prompt","message":"late permission reminder"}),
        ),
        (
            "allow",
            serde_json::json!({"hook_event_name":"PostToolUse","session_id":"permission-session","tool_name":"Write"}),
        ),
        (
            "deny",
            serde_json::json!({"hook_event_name":"PermissionDenied","session_id":"permission-session","tool_name":"Bash"}),
        ),
    ] {
        fs::write(dir.path().join(format!("{name}.json")), hook.to_string()).unwrap();
    }
    let provider = dir.path().join("claude");
    yinz_test_support::write_warm_script(
        &provider,
        r#"
"$@" < "$YINZ_PERMISSION_FIXTURE/start.json"
printf 'ready\n'
while read -r hook; do
  "$@" < "$YINZ_PERMISSION_FIXTURE/$hook.json" || exit 91
  printf '%s\n' "$hook"
done
"#,
    );
    let base = cli(&binary, yinz.session_name(), dir.path());
    let mut command = Command::new(&provider);
    command.arg(base.get_program()).args(base.get_args());
    for (key, value) in base.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        } else {
            command.env_remove(key);
        }
    }
    let child = command
        .args([
            "action",
            "agent-report",
            "--pane-id",
            &pane.to_string(),
            "--kind",
            "claude",
            "--hook-json",
        ])
        .env("YINZ_PERMISSION_FIXTURE", dir.path())
        .env("CLAUDE_CONFIG_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    processes.children.push(child);
    let pid = processes.children[0].id();
    let mut provider_output = BufReader::new(processes.children[0].stdout.take().unwrap());
    let mut line = String::new();
    provider_output.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "ready");
    let send_hook = |processes: &mut Processes,
                     output: &mut BufReader<std::process::ChildStdout>,
                     hook: &str| {
        writeln!(processes.children[0].stdin.as_mut().unwrap(), "{hook}").unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), hook);
    };
    let samples = dir.path().join("sessions").join(format!("{pid}.json"));
    fs::create_dir_all(samples.parent().unwrap()).unwrap();
    let mut timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let events = yinz.stream_events(&["agent_event", "pane_update"]);
    events.wait_for_event("PaneUpdate");
    let lines = Watched::new(Vec::<(Instant, serde_json::Value)>::new());
    let mut watch = cli(&binary, yinz.session_name(), dir.path())
        .args(["agent", "watch", "--json"])
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    processes.children.push(watch);
    let observed = lines.clone();
    processes.readers.push(std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                observed
                    .lock()
                    .push((Instant::now(), value["payload"].clone()));
            }
        }
    }));
    lines.wait_until_described(
        "CLI watch snapshot barrier",
        Duration::from_secs(10),
        |lines| {
            lines
                .iter()
                .any(|(_, v)| v["event_type"] == "ready")
                .then_some(())
        },
        |lines| format!("{lines:?}"),
    );
    registry(&samples, pid, "busy", "", timestamp);
    events.wait_for_event_matching("AgentEvent", |json| json.contains("ClaudePermissionStatus"));
    let status = || {
        let output = cli(&binary, yinz.session_name(), dir.path())
            .args(["agent", "status", &pane.to_string(), "--json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };

    // A real waiting prompt request demonstrates the CLI's needs_input outcome.
    let prompt = cli(&binary, yinz.session_name(), dir.path())
        .args([
            "agent",
            "prompt",
            &pane.to_string(),
            "review",
            "--submission",
            "permission-parent",
            "--wait",
            "--json",
        ])
        .spawn()
        .unwrap();
    processes.children.push(prompt);
    agent.wait_for_stdin("CLI prompt reached Claude", |bytes| {
        bytes.ends_with(b"\x1b[200~review\x1b[201~\r")
    });
    send_hook(&mut processes, &mut provider_output, "submit");

    for round in 1..=2 {
        send_hook(
            &mut processes,
            &mut provider_output,
            if round == 1 {
                "parent-request"
            } else {
                "worker-request"
            },
        );
        assert_eq!(status()["agents"][0]["agent"]["state"], "Busy");
        timestamp += 10;
        let began = Instant::now();
        let resolution =
            Resolution::after(samples.clone(), pid, timestamp + 1, Duration::from_secs(3));
        registry(&samples, pid, "waiting", "permission prompt", timestamp);
        let waiting_at = lines.wait_until(
            "CLI watch Waiting before independent approval",
            Duration::from_secs(1),
            |lines| {
                lines
                    .iter()
                    .find(|(at, v)| {
                        *at >= began
                            && v["event_type"] == "state_changed"
                            && v["state"] == "waiting"
                            && v["needs_input"] == true
                    })
                    .map(|(at, _)| *at)
            },
        );
        yinz.wait_until(
            "Scheme sees both accepted manifest and normalized permission event",
            |s| {
                s.contains(&format!(
                    "PERMISSION PW={round} EW={round} PR={} ER={}",
                    round - 1,
                    round - 1
                ))
            },
        );
        let waiting = status();
        assert_eq!(waiting["agents"][0]["agent"]["state"], "Waiting");
        assert_eq!(waiting["agents"][0]["agent"]["wait_needs_a_person"], true);
        assert!(waiting_at.duration_since(began) < Duration::from_secs(1));
        assert!(
            began.elapsed() < Duration::from_secs(1),
            "all consumers must observe the warmed prompt within one second"
        );
        assert!(
            !*resolution.done.lock(),
            "Waiting must precede independently timed approval"
        );
        if round == 1 {
            let child = &mut processes.children[2];
            let exit = child.wait().unwrap();
            assert_eq!(
                exit.code(),
                Some(1),
                "needs_input uses the CLI's non-success outcome code"
            );
            let mut bytes = String::new();
            use std::io::Read;
            child
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut bytes)
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&bytes).unwrap()["outcome"],
                "needs_input",
                "exit={exit:?} stdout={bytes}"
            );
        }
        resolution.done.wait_until(
            "independent three-second approval",
            Duration::from_secs(5),
            |done| (*done).then_some(()),
        );
        lines.wait_until(
            "CLI watch releases to current aggregate",
            Duration::from_secs(5),
            |lines| {
                lines
                    .iter()
                    .any(|(at, v)| {
                        *at > waiting_at
                            && v["event_type"] == "state_changed"
                            && v["state"] == "busy"
                    })
                    .then_some(())
            },
        );
        yinz.wait_until("Scheme sees release through both consumer paths", |s| {
            s.contains(&format!(
                "PERMISSION PW={round} EW={round} PR={round} ER={round}"
            ))
        });
        drop(resolution);
        send_hook(&mut processes, &mut provider_output, "late");
        assert_eq!(
            status()["agents"][0]["agent"]["state"],
            "Busy",
            "a delayed notification must not resurrect an answered dialog"
        );
        if round == 1 {
            let answer = serde_json::json!({"type":"assistant","parentUuid":"user","uuid":"answer","message":{"content":[{"type":"text","text":"parent done"}]}});
            let end = serde_json::json!({"type":"system","subtype":"turn_duration","parentUuid":"answer"});
            fs::write(&transcript, format!("{row}\n{answer}\n{end}\n")).unwrap();
            events.wait_for_event_matching("AgentEvent", |json| {
                json.contains("_yinz_claude_turn_complete") && json.contains("parent done")
            });
            assert_eq!(status()["agents"][0]["background"], true);
        }
    }
    // Automatic allow/deny and evaluation beyond 500ms have no positive human reason.
    let waiting_count = lines
        .lock()
        .iter()
        .filter(|(_, v)| v["event_type"] == "state_changed" && v["state"] == "waiting")
        .count();
    for (resolution_hook, duration) in [
        ("allow", Duration::ZERO),
        ("deny", Duration::ZERO),
        ("allow", Duration::from_millis(800)),
    ] {
        timestamp += 10;
        registry(&samples, pid, "busy", "", timestamp);
        send_hook(&mut processes, &mut provider_output, "parent-request");
        let barrier = Resolution::after(samples.clone(), pid, timestamp + 1, duration);
        barrier.done.wait_until(
            "automatic evaluation finishes",
            Duration::from_secs(5),
            |done| (*done).then_some(()),
        );
        send_hook(&mut processes, &mut provider_output, resolution_hook);
        send_hook(&mut processes, &mut provider_output, "late");
        assert_eq!(status()["agents"][0]["agent"]["state"], "Busy");
    }
    assert_eq!(
        lines
            .lock()
            .iter()
            .filter(|(_, v)| v["event_type"] == "state_changed" && v["state"] == "waiting")
            .count(),
        waiting_count
    );
    assert_eq!(
        waiting_count, 2,
        "heartbeats/reminders produce one Waiting edge per dialog"
    );
    // Explicitly tear down external children before stopping the server.
    drop(processes);
    yinz.quit();
}

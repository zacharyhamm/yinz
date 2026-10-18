//! Process-level acceptance: stdin, JSON, live background services, and installation.
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Output, Stdio},
    sync::{Arc, Mutex},
};
use yinz_types::plugin_data::{DataRows, DataValue, FetchRequest, Mutation, Pred};

fn integer(value: Option<&DataValue>) -> i64 {
    match value {
        Some(DataValue::Int(value)) => *value,
        _ => panic!("expected integer: {value:?}"),
    }
}
fn command(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_yinz"));
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "YINZ_SOCKET_DIR",
        "TMPDIR",
    ] {
        cmd.env(key, dir);
    }
    for key in ["YINZ", "YINZ_SESSION_NAME", "YINZ_PANE_ID", "YINZ_PANE_REF"] {
        cmd.env_remove(key);
    }
    cmd.arg("--data-dir")
        .arg(dir.join("yinz"))
        .current_dir(dir)
        .env("SHELL", "/bin/sh")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}
fn success(out: Output) -> Value {
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("exactly one JSON value")
}
fn failure(out: Output, expected: &str) {
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(expected),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
struct Session(tempfile::TempDir, String, Option<AttachedClient>);
const ATTACH_OUTPUT_LIMIT: usize = 32 * 1024;

fn capture_tail(mut reader: impl Read, output: &Mutex<Vec<u8>>) {
    let mut buffer = [0; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                let mut output = output.lock().unwrap();
                let excess = (output.len() + count).saturating_sub(ATTACH_OUTPUT_LIMIT);
                output.drain(..excess);
                output.extend_from_slice(&buffer[..count]);
            },
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn attachment_logs(root: &Path, output: &mut Vec<u8>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            attachment_logs(&path, output);
        } else if path.file_name().is_some_and(|name| name == "yinz.log") {
            use std::io::{Seek, SeekFrom};
            if let Ok(mut file) = std::fs::File::open(&path) {
                let _ = file.seek(SeekFrom::End(-(ATTACH_OUTPUT_LIMIT as i64)));
                let tail = Mutex::new(std::mem::take(output));
                capture_tail(file, &tail);
                *output = tail.into_inner().unwrap();
            }
        }
    }
}

struct AttachedClient(
    std::process::Child,
    Option<std::thread::JoinHandle<()>>,
    Arc<Mutex<Vec<u8>>>,
);
impl AttachedClient {
    fn new(dir: &Path) -> Self {
        let pty = nix::pty::openpty(
            Some(&nix::pty::Winsize {
                ws_row: 60,
                ws_col: 160,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None,
        )
        .unwrap();
        let slave = std::fs::File::from(pty.slave);
        let child = command(dir)
            .args([
                "--config-dir",
                dir.to_str().unwrap(),
                "attach",
                "portable-test",
            ])
            .env("TERM", "xterm-256color")
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave)
            .spawn()
            .unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let captured = output.clone();
        let reader = std::thread::spawn(move || {
            capture_tail(std::fs::File::from(pty.master), &captured);
        });
        Self(child, Some(reader), output)
    }
}
impl Drop for AttachedClient {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        if let Some(reader) = self.1.take() {
            reader.join().unwrap();
        }
    }
}
impl Session {
    fn new() -> Self {
        Self::start(|_| {}, false)
    }
    fn with_setup(setup: impl FnOnce(&Path)) -> Self {
        Self::start(setup, true)
    }
    fn start(setup: impl FnOnce(&Path), attach: bool) -> Self {
        let mut session = Self(tempfile::tempdir().unwrap(), String::new(), None);
        let dir = session.0.path();
        yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &dir.join("yinz")).unwrap();
        assert!(Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg(dir)
            .status()
            .unwrap()
            .success());
        std::fs::write(dir.join("config.kdl"), "plugins { project-store location=\"yinz:project-store\" { shared true; }; agent-service location=\"yinz:agent-service\" { shared true; }; }\nload_plugins clear-defaults=true { \"project-store\"; \"agent-service\"; }\ndetachable_panes false\n").unwrap();
        std::fs::write(dir.join("layout.kdl"), "layout { pane command=\"/bin/sh\" { args \"-c\" \"printf 'PORTABLE READY\\n'; exec /bin/sh\"; }; }\n").unwrap();
        setup(dir);
        let out = command(dir)
            .args([
                "--config-dir",
                dir.to_str().unwrap(),
                "--config",
                "config.kdl",
                "--layout",
                "layout.kdl",
                "attach",
                "--create-background",
                "portable-test",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let pane = session.run(
            &[
                "action",
                "new-pane",
                "--",
                "/bin/sh",
                "-c",
                "printf 'PORTABLE READY\n'; exec /bin/sh",
            ],
            None,
        );
        assert!(
            pane.status.success(),
            "{}",
            String::from_utf8_lossy(&pane.stderr)
        );
        session.1 = String::from_utf8(pane.stdout).unwrap().trim().to_owned();
        success(session.run(
            &[
                "action",
                "pane-wait",
                &session.1,
                "--contains",
                "PORTABLE READY",
            ],
            None,
        ));
        // Wait for the background session to serve pane actions before probing attachment.
        if attach {
            session.2 = Some(AttachedClient::new(session.0.path()));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                let response = session.run(&["action", "list-clients"], None);
                let client = session.2.as_mut().unwrap();
                let status = client.0.try_wait().unwrap();
                let response_text = std::str::from_utf8(&response.stdout);
                let attached = response_text.is_ok_and(|text| text.trim().lines().count() >= 2);
                if status.is_some()
                    || !response.status.success()
                    || !response.stderr.is_empty()
                    || response_text.is_err()
                    || (!attached && std::time::Instant::now() >= deadline)
                {
                    let _ = client.0.kill();
                    let final_status = client.0.wait().unwrap();
                    client.1.take().unwrap().join().unwrap();
                    let mut logs = Vec::new();
                    attachment_logs(session.0.path(), &mut logs);
                    panic!(
                        "terminal client failed to attach: {final_status}; observed exit: {status:?}\nterminal output:\n{}\nlist-clients: {}\nstdout: {}\nstderr: {}\nserver logs:\n{}",
                        String::from_utf8_lossy(&client.2.lock().unwrap()),
                        response.status,
                        String::from_utf8_lossy(&response.stdout),
                        String::from_utf8_lossy(&response.stderr),
                        String::from_utf8_lossy(&logs),
                    );
                }
                if attached {
                    break;
                }
            }
        }
        session
    }
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = command(self.0.path());
        cmd.args([
            "--config-dir",
            self.0.path().to_str().unwrap(),
            "--session",
            "portable-test",
        ])
        .args(args);
        cmd
    }
    fn run(&self, args: &[&str], input: Option<&str>) -> Output {
        eprintln!("CLI {args:?}");
        let mut cmd = self.cmd(args);
        if let Some(input) = input {
            let mut child = cmd.stdin(Stdio::piped()).spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        } else {
            cmd.output().unwrap()
        }
    }
    fn fetch(&self, request: FetchRequest) -> DataRows {
        serde_json::from_value(success(self.run(
            &["data", "fetch", "--namespace", "shared:yinz", "--file", "-"],
            Some(&serde_json::to_string(&request).unwrap()),
        )))
        .unwrap()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = command(self.0.path())
            .args(["kill-session", "portable-test"])
            .output();
        self.2.take();
        if std::thread::panicking() {
            let mut logs = Vec::new();
            attachment_logs(self.0.path(), &mut logs);
            eprintln!("server logs:\n{}", String::from_utf8_lossy(&logs));
        }
    }
}

#[test]
fn repl_connects_automatically_and_preserves_input_and_output_behavior() {
    let session = Session::with_setup(|dir| {
        let config = dir.join("config.kdl");
        let source = std::fs::read_to_string(&config).unwrap();
        std::fs::write(
            config,
            source.replace("detachable_panes false", "detachable_panes true"),
        )
        .unwrap();
    });
    let expression = r#"(define id (hash-ref (yinz-command "GetPluginIds") "plugin_id")) (if (number? id) (+ 20 22) (error "missing plugin ID"))"#;
    let output = command(session.0.path())
        .args(["repl", "--eval", expression])
        .output()
        .unwrap();
    assert_eq!(success(output), json!(42));
    assert_eq!(
        success(session.run(&["repl"], Some("(define answer 6)\n(* answer 7)\n"))),
        json!(42)
    );
    for input in [None, Some("(display \"before\") (error \"failed\")")] {
        let args = if input.is_some() {
            vec!["repl"]
        } else {
            vec!["repl", "--eval", "(display \"before\") (error \"failed\")"]
        };
        let output = session.run(&args, input);
        assert!(!output.status.success());
        assert_eq!(output.stdout, b"before");
        assert!(String::from_utf8_lossy(&output.stderr).contains("failed"));
    }

    // A real pane supplies session metadata and a terminal for interactive input.
    let output = session.run(
        &[
            "action",
            "new-pane",
            "--",
            env!("CARGO_BIN_EXE_yinz"),
            "repl",
        ],
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let pane = String::from_utf8(output.stdout).unwrap();
    let pane = pane.trim();
    success(session.run(
        &[
            "action",
            "pane-wait",
            pane,
            "--contains",
            "Connected to Yinz session portable-test.",
        ],
        None,
    ));
    let terminal = pane.strip_prefix("terminal_").unwrap();
    let output = session.run(
        &[
            "action",
            "write-chars",
            "--pane-id",
            terminal,
            "(define answer\n  42)\nmissing-name\n(display \"answer=\") (+ answer 1)\n",
        ],
        None,
    );
    assert!(output.status.success());
    success(session.run(
        &["action", "pane-wait", pane, "--contains", "answer=43"],
        None,
    ));
    success(session.run(&["action", "pane-wait", pane, "--contains", "Error:"], None));
    assert!(session
        .run(
            &["action", "write-chars", "--pane-id", terminal, ":quit\n"],
            None,
        )
        .status
        .success());
}

#[test]
fn background_operations_data_revisions_and_waits_work_without_chat() {
    let session = Session::new();
    let pane = session.1.as_str();
    let id = pane.strip_prefix("terminal_").unwrap();
    let context = success(session.run(&["action", "context"], None));
    assert!(
        context["pane"].is_null(),
        "outside-pane discovery must not guess"
    );
    assert!(context["missing"]
        .as_array()
        .unwrap()
        .contains(&json!("caller_pane")));
    success(session.run(
        &[
            "action",
            "project-remember",
            "work",
            "--root",
            session.0.path().to_str().unwrap(),
        ],
        None,
    ));
    success(session.run(&["action", "project-assign", "work", "--tab-id", "0"], None));
    assert_eq!(
        success(session.run(&["action", "context", "--pane-id", id], None))["project"],
        "work"
    );
    success(session.run(&["agent", "register", id, "reviewer"], None));
    success(session.run(
        &[
            "action",
            "project-remember",
            "other",
            "--root",
            "/work/other",
        ],
        None,
    ));
    success(session.run(
        &["action", "project-assign", "other", "--tab-id", "0"],
        None,
    ));
    let assigned = success(session.run(&["agent", "list", "--project", "other", "--json"], None));
    assert!(
        assigned["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|agent| agent["pane_id"] == pane),
        "explicit tab assignment must outrank cwd discovery"
    );
    success(session.run(&["action", "project-assign", "work", "--tab-id", "0"], None));
    success(session.run(&["action", "project-forget", "other"], None));
    assert!(!session
        .run(
            &[
                "agent",
                "wait",
                pane,
                "--until",
                "idle",
                "--timeout",
                "1ms",
                "--json"
            ],
            None
        )
        .status
        .success());
    let waiter = session
        .cmd(&["action", "pane-wait", pane, "--contains", "CLI_CHANGED"])
        .spawn()
        .unwrap();
    assert!(session
        .run(
            &[
                "action",
                "write-chars",
                "--pane-id",
                pane,
                "printf CLI_CHANGED\n"
            ],
            None
        )
        .status
        .success());
    let waited = waiter.wait_with_output().unwrap();
    if !waited.status.success() {
        eprintln!(
            "pane dump: {}",
            String::from_utf8_lossy(
                &session
                    .run(&["action", "dump-screen", "--pane-id", pane], None)
                    .stdout
            )
        );
    }
    success(waited);
    failure(
        session.run(
            &[
                "action",
                "pane-wait",
                pane,
                "--contains",
                "never produced marker",
                "--timeout",
                "1",
            ],
            None,
        ),
        "deadline",
    );
    failure(
        session.run(
            &["action", "pane-wait", "terminal_99999", "--contains", "x"],
            None,
        ),
        "not found",
    );
    success(session.run(&["action", "pane-wait", "terminal_99999", "--closed"], None));

    let rows = session.fetch(FetchRequest::entity("project").select(["id", "rev", "name", "root"]));
    let row = &rows.rows[0];
    let row_id = integer(rows.get(row, "id"));
    let rev = integer(rows.get(row, "rev"));
    let guarded = |revision| json!({"Update":{"entity":"project","filter":{"Eq":["id",{"Int":row_id}]},"values":[["worktree_dir",{"Text":"trees"}]],"expect_rev":revision}});
    let apply = |mutations: Value| {
        session.run(
            &["data", "apply", "--namespace", "shared:yinz", "--file", "-"],
            Some(&mutations.to_string()),
        )
    };
    success(apply(json!([guarded(rev)])));
    failure(apply(json!([guarded(rev)])), "revision");
    let fresh =
        session.fetch(FetchRequest::entity("project").select(["id", "rev", "worktree_dir"]));
    let now = integer(fresh.get(&fresh.rows[0], "rev"));
    failure(apply(json!([guarded(now), guarded(999999)])), "revision");
    assert_eq!(
        session.fetch(FetchRequest::entity("project").select(["id", "rev", "worktree_dir"])),
        fresh,
        "failed batch rolls back the first update too"
    );
    let deletion = Mutation::Delete {
        entity: "project".into(),
        filter: Pred::eq("id", row_id),
    };
    failure(apply(json!([deletion])), "AssertRows");
    let config = success(session.run(&["action", "project-config-read", "work"], None));
    assert!(config["source"].is_null());
    assert_eq!(config["config"]["sections"], json!({}));
    let source = "agent_exit { default_branch \"main\"; }\nmerge_queue { ci_command \"cargo x ci check\"; }\n";
    let written = success(session.run(
        &["action", "project-config-write", "work", "--file", "-"],
        Some(&json!({"source": source, "expected": null}).to_string()),
    ));
    let config = success(session.run(&["action", "project-config-read", "work"], None));
    assert_eq!(config, written);
    assert_eq!(config["source"], source);
    assert_eq!(
        config["config"]["sections"]["merge_queue"]["children"][0]["arguments"],
        json!(["cargo x ci check"])
    );
    failure(
        session.run(
            &["action", "project-config-write", "work", "--file", "-"],
            Some("{\"source\":\"\",\"expected\":null}"),
        ),
        "changed",
    );
    success(session.run(&["action", "project-rename", "work", "renamed"], None));
    assert_eq!(
        success(session.run(&["action", "context", "--pane-id", id], None))["project"],
        "renamed"
    );
    let listed = success(session.run(&["action", "project-list"], None));
    let renamed = listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|project| project["name"] == "renamed")
        .expect("renamed project is listed");
    assert_eq!(
        Path::new(renamed["root"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        session.0.path().canonicalize().unwrap()
    );
    assert!(!listed["live"].is_null());
    let catalog = success(session.run(&["data", "catalog"], None));
    assert!(catalog
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity["namespace"] == "shared:yinz"));
    let runs = success(session.run(&["agent", "runs", "--project", "renamed"], None));
    assert!(runs["runs"].is_array());
    failure(
        session.run(&["agent", "run", "missing-run"], None),
        "no such run",
    );
    let panes = success(session.run(&["action", "list-panes", "--json"], None));
    assert!(!panes
        .as_array()
        .unwrap()
        .iter()
        .any(|pane| pane["plugin_url"] == "agents"));

    assert!(Command::new("git")
        .current_dir(session.0.path())
        .args([
            "-c",
            "user.name=CLI Test",
            "-c",
            "user.email=cli@example.invalid",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "fixture"
        ])
        .status()
        .unwrap()
        .success());
    let executable = session.0.path().join("codex");
    yinz_test_support::write_warm_script(
        &executable,
        "#!/bin/sh\nprintf AGENT_READY\nexec /bin/sh\n",
    );
    let spawned = success(
        session.run(
            &["agent", "spawn", "--file", "-", "--json"],
            Some(
                &json!({
                    "command":executable, "hooks":false,
                    "project":"renamed", "branch":"review", "base":"HEAD"
                })
                .to_string(),
            ),
        ),
    );
    let spawned = &spawned["spawned"];
    let spawned_pane = spawned["pane_id"].as_str().unwrap().to_owned();
    assert_eq!(spawned["branch"], "review");
    assert!(Path::new(spawned["cwd"].as_str().unwrap())
        .join(".git")
        .is_file());
    success(session.run(
        &[
            "action",
            "pane-wait",
            &spawned_pane,
            "--contains",
            "AGENT_READY",
        ],
        None,
    ));
    let spawned_id = spawned_pane.strip_prefix("terminal_").unwrap();
    success(session.run(&["agent", "register", spawned_id, "reviewer"], None));
    success(session.run(&["agent", "read", &spawned_pane, "--json"], None));
    // Reload the metadata owner with no chat loaded, then query its persisted assignment.
    assert!(session
        .run(
            &["action", "start-or-reload-plugin", "yinz:agent-service"],
            None
        )
        .status
        .success());
    assert_eq!(
        success(session.run(&["action", "context", "--pane-id", id], None))["project"],
        "renamed"
    );
    assert!(session
        .run(&["action", "close-pane", "--pane-id", &spawned_pane], None)
        .status
        .success());
    success(session.run(&["action", "pane-wait", &spawned_pane, "--closed"], None));
}

#[test]
fn checked_plugin_edits_and_installed_tests_work_without_chat() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("yinz")).unwrap();
    let plugin_command = || {
        let mut cmd = command(dir.path());
        cmd.arg("--config-dir").arg(dir.path().join("yinz"));
        cmd
    };
    let source = dir.path().join("source.scm");
    std::fs::write(&source, r#"(define (render rows columns) "FIRST")"#).unwrap();
    let out = plugin_command()
        .args(["plugins", "write", "fixture.scm", "--file"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let installed = std::path::PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
    let edits = dir.path().join("edits.json");
    let edit = || {
        plugin_command()
            .args(["plugins", "edit", "fixture.scm", "--file"])
            .arg(&edits)
            .output()
            .unwrap()
    };
    std::fs::write(&edits, r#"[{"old_string":"FIRST","new_string":"SECOND"}]"#).unwrap();
    success(edit());
    let saved = std::fs::read_to_string(&installed).unwrap();
    assert!(saved.contains("SECOND"));
    assert!(installed.with_file_name("fixture.bak.scm").is_file());
    std::fs::write(&edits, r#"[{"old_string":"SECOND","new_string":"THIRD"},{"old_string":"MISSING","new_string":"bad"}]"#).unwrap();
    assert!(!edit().status.success());
    assert_eq!(std::fs::read_to_string(&installed).unwrap(), saved);
    std::fs::write(installed.with_file_name("fixture.test.scm"), r#"(test "renders edited source" (lambda () (test-load-plugin "./fixture.scm") (assert-equal (test-render) "SECOND")))"#).unwrap();
    let report = success(
        plugin_command()
            .args(["plugins", "test", "fixture.scm", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["tests"][0]["status"], "passed");
}

fn output_text(out: Output) -> String {
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

struct GitPlugins {
    directory: tempfile::TempDir,
    server: Option<std::process::Child>,
}
impl GitPlugins {
    fn new() -> Self {
        let fixture = Self {
            directory: tempfile::tempdir().unwrap(),
            server: None,
        };
        fixture.git(&["init", "--quiet", "-b", "main"]);
        std::fs::create_dir(fixture.directory.path().join("nested")).unwrap();
        fixture.write("v1");
        fixture.commit();
        fixture.git(&["tag", "v1"]);
        fixture.git(&["branch", "feature"]);
        fixture
    }
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(self.directory.path())
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Plugin test",
                "-c",
                "user.email=plugin@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
    fn write(&self, version: &str) {
        std::fs::write(self.directory.path().join("main.scm"), format!(r#"
(define greeting "")
(define (load config) (when (hash-contains? config "greeting") (set! greeting (hash-ref config "greeting"))))
(define (render rows columns) (string-append "hello {version} " greeting))
"#)).unwrap();
        std::fs::write(
            self.directory.path().join("shared.scm"),
            format!(r#"(provide version) (define version "{version}")"#),
        )
        .unwrap();
        std::fs::write(self.directory.path().join("nested/main.scm"), r#"
(require "../shared.scm")
(define greeting "")
(define (load config) (when (hash-contains? config "greeting") (set! greeting (hash-ref config "greeting"))))
(define (render rows columns) (string-append "nested " version " " greeting))
"#).unwrap();
    }
    fn commit(&self) -> String {
        self.git(&["add", "."]);
        self.git(&["commit", "--quiet", "-m", "fixture version"]);
        self.git(&["rev-parse", "HEAD"])
    }
    fn serve(&mut self) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        self.server = Some(
            Command::new("git")
                .process_group(0)
                .args([
                    "daemon",
                    "--listen=127.0.0.1",
                    &format!("--port={port}"),
                    "--export-all",
                    "--reuseaddr",
                    &format!("--base-path={}", self.directory.path().display()),
                    self.directory.path().to_str().unwrap(),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "Git daemon did not start"
            );
            std::thread::yield_now();
        }
        format!("git://127.0.0.1:{port}/")
    }
    fn stop(&mut self) {
        if let Some(mut server) = self.server.take() {
            // Git spawns git-daemon, which can spawn connection workers.
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(server.id() as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = server.wait();
        }
    }
}
impl Drop for GitPlugins {
    fn drop(&mut self) {
        self.stop();
    }
}

fn plugin_config(dir: &Path, repo: &str, pins: Option<&str>, layout: bool) {
    std::fs::write(dir.join("config.kdl"), format!("load_plugins clear-defaults=true {{}}\ndetachable_panes false\nshow_release_notes false\nplugins {{ hello repo=\"{repo}\" {{ greeting \"CONFIGURED\"; }}; }}\n")).unwrap();
    std::fs::create_dir(dir.join("config.d")).unwrap();
    let mut aliases = format!("plugins {{ nested repo=\"{repo}\" entrypoint=\"nested/main.scm\" {{ greeting \"NESTED_CONFIG\"; }}; ");
    if let Some(commit) = pins {
        aliases.push_str(&format!("branch repo=\"{repo}\" branch=\"feature\" {{ greeting \"BRANCH_CONFIG\"; }}; tagged repo=\"{repo}\" tag=\"v1\" {{ greeting \"TAG_CONFIG\"; }}; pinned repo=\"{repo}\" commit=\"{commit}\" {{ greeting \"COMMIT_CONFIG\"; }}; "));
    }
    aliases.push_str("}\n");
    std::fs::write(dir.join("config.d/plugins.kdl"), aliases).unwrap();
    if layout {
        std::fs::write(
            dir.join("layout.kdl"),
            "layout { pane { plugin location=\"hello\"; }; pane command=\"/bin/sh\"; }\n",
        )
        .unwrap();
    }
}

impl Session {
    fn plugin_cli(&self, args: &[&str]) -> String {
        output_text(self.run(args, None))
    }
    fn plugin_pane(&self, alias: &str) -> String {
        let panes = success(self.run(&["action", "list-panes", "--json"], None));
        let matches = panes
            .as_array()
            .unwrap()
            .iter()
            .filter(|pane| pane["plugin_url"] == alias)
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "{panes}");
        format!("plugin_{}", matches[0]["id"].as_u64().unwrap())
    }
    fn rendered(&self, pane: &str, content: &str) {
        success(self.run(&["action", "pane-wait", pane, "--contains", content], None));
    }
    fn load_plugin(&self, alias: &str) -> String {
        self.plugin_cli(&["action", "launch-or-focus-plugin", alias])
    }
    fn reload_plugin(&self, alias: &str, pane: &str, content: &str) {
        self.plugin_cli(&["plugins", "reload", alias]);
        self.rendered(pane, content);
        assert_eq!(self.plugin_pane(alias), pane);
    }
}

#[test]
fn local_git_plugin_cli_reload_preserves_dirty_sources_and_configuration() {
    let fixture = GitPlugins::new();
    let session = Session::with_setup(|dir| {
        plugin_config(dir, fixture.directory.path().to_str().unwrap(), None, true)
    });
    let hello = session.plugin_pane("hello");
    session.rendered(&hello, "hello v1 CONFIGURED");
    let nested = session.load_plugin("nested");
    session.rendered(&nested, "nested v1 NESTED_CONFIG");
    for alias in ["hello", "nested"] {
        assert!(session
            .plugin_cli(&["plugins", "install", alias])
            .contains("user-managed"));
        assert!(session
            .plugin_cli(&["plugins", "status", alias])
            .contains("user-managed"));
        assert!(
            success(session.run(&["plugins", "check", alias, "--json"], None))["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    failure(
        session.run(&["plugins", "install", "unknown"], None),
        "not a configured repository plugin",
    );
    fixture.write("v2");
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let status = fixture.git(&["status", "--porcelain=v1"]);
    let diff = fixture.git(&["diff", "HEAD"]);
    let index = std::fs::read(fixture.directory.path().join(".git/index")).unwrap();
    session.plugin_cli(&["plugins", "update"]);
    session.rendered(&hello, "hello v1 CONFIGURED");
    session.rendered(&nested, "nested v1 NESTED_CONFIG");
    session.reload_plugin("hello", &hello, "hello v2 CONFIGURED");
    session.reload_plugin("nested", &nested, "nested v2 NESTED_CONFIG");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fixture.git(&["status", "--porcelain=v1"]), status);
    assert_eq!(fixture.git(&["diff", "HEAD"]), diff);
    assert_eq!(
        std::fs::read(fixture.directory.path().join(".git/index")).unwrap(),
        index
    );
    assert!(!session.0.path().join("yinz/plugin-repositories").exists());

    std::fs::write(session.0.path().join("config.d/plugins.kdl"), format!("plugins {{ nested repo=\"{}\" entrypoint=\"main.scm\" {{ greeting \"REFRESHED\"; }}; }}\n", fixture.directory.path().display())).unwrap();
    assert!(session
        .plugin_cli(&["plugins", "status", "nested"])
        .contains("entrypoint=main.scm"));
    // The config watcher is asynchronous; retry future launches until its new
    // declaration renders, waiting on content rather than a fixed delay.
    session.plugin_cli(&["action", "close-pane", "--pane-id", &nested]);
    success(session.run(&["action", "pane-wait", &nested, "--closed"], None));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let refreshed = session.load_plugin("nested");
        let out = session.run(
            &[
                "action",
                "pane-wait",
                &refreshed,
                "--contains",
                "hello v2 REFRESHED",
                "--timeout",
                "1",
            ],
            None,
        );
        if out.status.success() {
            success(out);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "config drop-in did not reach future launches"
        );
        session.plugin_cli(&["action", "close-pane", "--pane-id", &refreshed]);
        success(session.run(&["action", "pane-wait", &refreshed, "--closed"], None));
    }
}

#[test]
fn remote_git_plugin_cli_install_update_reload_pins_and_offline() {
    let mut fixture = GitPlugins::new();
    let v1 = fixture.git(&["rev-parse", "HEAD"]);
    let remote = fixture.serve();
    let session = Session::with_setup(|dir| plugin_config(dir, &remote, Some(&v1), false));
    assert!(session
        .plugin_cli(&["plugins", "status", "hello"])
        .contains("not installed"));
    failure(
        session.run(&["plugins", "install", "unknown"], None),
        "not a configured repository plugin",
    );
    assert!(session.plugin_cli(&["plugins", "install"]).contains(&v1));
    for alias in ["hello", "nested", "branch", "tagged", "pinned"] {
        assert!(session
            .plugin_cli(&["plugins", "status", alias])
            .contains(&v1));
        success(session.run(&["plugins", "check", alias, "--json"], None));
    }
    let hello = session.load_plugin("hello");
    let nested = session.load_plugin("nested");
    let branch = session.load_plugin("branch");
    session.rendered(&hello, "hello v1 CONFIGURED");
    session.rendered(&nested, "nested v1 NESTED_CONFIG");
    session.rendered(&branch, "hello v1 BRANCH_CONFIG");
    // A separate server and data directory exercise installation triggered by a layout.
    let first_load = Session::with_setup(|dir| plugin_config(dir, &remote, None, true));
    let first_pane = first_load.plugin_pane("hello");
    first_load.rendered(&first_pane, "hello v1 CONFIGURED");
    assert!(first_load
        .plugin_cli(&["plugins", "status", "hello"])
        .contains(&v1));
    drop(first_load);

    fixture.write("v2");
    let v2 = fixture.commit();
    fixture.git(&["branch", "-f", "feature", &v2]);
    session.plugin_cli(&["plugins", "install"]);
    for alias in ["hello", "nested", "branch", "tagged", "pinned"] {
        assert!(session
            .plugin_cli(&["plugins", "status", alias])
            .contains(&v1));
    }
    session.plugin_cli(&["plugins", "update"]);
    for alias in ["hello", "nested", "branch"] {
        assert!(session
            .plugin_cli(&["plugins", "status", alias])
            .contains(&v2));
    }
    session.rendered(&hello, "hello v1 CONFIGURED");
    session.rendered(&nested, "nested v1 NESTED_CONFIG");
    session.rendered(&branch, "hello v1 BRANCH_CONFIG");
    session.reload_plugin("hello", &hello, "hello v2 CONFIGURED");
    session.reload_plugin("nested", &nested, "nested v2 NESTED_CONFIG");
    session.reload_plugin("branch", &branch, "hello v2 BRANCH_CONFIG");
    for (alias, greeting) in [("tagged", "TAG_CONFIG"), ("pinned", "COMMIT_CONFIG")] {
        assert!(session
            .plugin_cli(&["plugins", "status", alias])
            .contains(&v1));
        let pane = session.load_plugin(alias);
        session.reload_plugin(alias, &pane, &format!("hello v1 {greeting}"));
    }

    std::fs::remove_file(fixture.directory.path().join("nested/main.scm")).unwrap();
    fixture.commit();
    failure(
        session.run(&["plugins", "update", "hello"], None),
        "resolve repository entrypoint",
    );
    assert!(session
        .plugin_cli(&["plugins", "status", "hello"])
        .contains(&v2));
    session.reload_plugin("nested", &nested, "nested v2 NESTED_CONFIG");
    fixture.write("v3");
    let v3 = fixture.commit();
    session.plugin_cli(&["plugins", "update", "hello"]);
    assert!(session
        .plugin_cli(&["plugins", "status", "hello"])
        .contains(&v3));
    session.reload_plugin("hello", &hello, "hello v3 CONFIGURED");
    session.reload_plugin("nested", &nested, "nested v3 NESTED_CONFIG");
    fixture.stop();
    assert!(session
        .plugin_cli(&["plugins", "status", "hello"])
        .contains(&v3));
    session.plugin_cli(&["plugins", "install", "hello"]);
    session.reload_plugin("hello", &hello, "hello v3 CONFIGURED");
    session.plugin_cli(&["action", "close-pane", "--pane-id", &nested]);
    success(session.run(&["action", "pane-wait", &nested, "--closed"], None));
    let offline = session.load_plugin("nested");
    session.rendered(&offline, "nested v3 NESTED_CONFIG");
}

#[test]
fn bundle_installation_preserves_unrelated_files_and_preflights_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("custom agent/skills");
    let install = |args: &[&str]| {
        command(dir.path())
            .args(["skills", "install", "--dest"])
            .arg(&dest)
            .env("CODEX_HOME", dir.path().join(".codex"))
            .args(args)
            .output()
            .unwrap()
    };
    assert!(install(&["--agent", "codex", "--dry-run"]).status.success());
    assert!(!dest.exists());
    assert!(install(&["--agent", "codex"]).status.success());
    for root in [".claude", ".codex", "opencode", "muse", ".gemini"] {
        assert!(
            !dir.path().join(root).exists(),
            "unexpected install: {root}"
        );
    }
    let names = success(
        command(dir.path())
            .args(["skills", "list"])
            .output()
            .unwrap(),
    );
    assert_eq!(names.as_array().unwrap().len(), 5);
    for name in names.as_array().unwrap() {
        let path = dest.join(name.as_str().unwrap()).join("SKILL.md");
        let shown = command(dir.path())
            .args(["skills", "show", name.as_str().unwrap()])
            .output()
            .unwrap();
        assert!(shown.status.success());
        assert_eq!(std::fs::read(path).unwrap(), shown.stdout);
    }
    let extra = dest.join("yinz-agents/personal.md");
    std::fs::write(&extra, "mine").unwrap();
    assert!(install(&[]).status.success());
    let changed = dest.join("yinz-agents/SKILL.md");
    std::fs::write(&changed, "local edits").unwrap();
    std::fs::remove_file(dest.join("yinz-sessions/SKILL.md")).unwrap();
    assert!(!install(&[]).status.success());
    assert!(!dest.join("yinz-sessions/SKILL.md").exists());
    assert!(install(&["--force", "--dry-run"]).status.success());
    assert_eq!(std::fs::read_to_string(&changed).unwrap(), "local edits");
    assert!(install(&["--force"]).status.success());
    assert_eq!(std::fs::read_to_string(extra).unwrap(), "mine");
}

#[test]
fn plugin_check_targets_have_text_json_parity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("space name.scm");
    std::fs::write(
        root.join("config.kdl"),
        format!(
            "plugins {{ selected location=\"file:{}\"; }}\n",
            path.display()
        ),
    )
    .unwrap();
    let targets = [
        path.display().to_string(),
        "./space name.scm".into(),
        format!(
            "../{}/space name.scm",
            root.file_name().unwrap().to_str().unwrap()
        ),
        format!("file:{}", path.display()),
        format!("file://{}", path.display()),
        format!("file://{}", path.display()).replace(' ', "%20"),
        "file:./space name.scm".into(),
        "selected".into(),
    ];
    for (source, valid) in [("(define value 1)", true), ("(define value", false)] {
        std::fs::write(&path, source).unwrap();
        for target in &targets {
            for json in [false, true] {
                let mut cmd = command(&root);
                cmd.args(["--config", "config.kdl", "plugins", "check", target]);
                if json {
                    cmd.arg("--json");
                }
                let out = cmd.output().unwrap();
                assert_eq!(
                    out.status.success(),
                    valid,
                    "{target}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                if json {
                    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
                    assert_eq!(report["diagnostics"].as_array().unwrap().is_empty(), valid);
                } else if valid {
                    assert!(String::from_utf8_lossy(&out.stdout).contains("checks successfully"));
                }
            }
        }
    }
    std::fs::write(
        &path,
        "(define (ask permissions) (yinz-wait-for-permissions permissions))",
    )
    .unwrap();
    for json in [false, true] {
        let mut cmd = command(&root);
        cmd.args(["--config", "config.kdl", "plugins", "check", "selected"]);
        if json {
            cmd.arg("--json");
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        if json {
            let report: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert!(!report["warnings"].as_array().unwrap().is_empty());
            assert!(out.stderr.is_empty());
        } else {
            assert!(String::from_utf8_lossy(&out.stderr).contains("computed permission"));
            assert!(!String::from_utf8_lossy(&out.stdout).contains("computed permission"));
        }
    }
    std::fs::remove_file(path).unwrap();
    for target in &targets {
        for json in [false, true] {
            let mut cmd = command(&root);
            cmd.args(["--config", "config.kdl", "plugins", "check", target]);
            if json {
                cmd.arg("--json");
            }
            let out = cmd.output().unwrap();
            assert!(!out.status.success());
            assert!(out.stdout.is_empty());
            assert!(!out.stderr.is_empty());
        }
    }
}

#[test]
fn plugin_cli_targets_resolve_managed_names_and_explicit_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let managed = root.join("managed");
    std::fs::create_dir_all(managed.join("folder")).unwrap();
    std::fs::create_dir(root.join("folder")).unwrap();
    for name in [
        "foo.scm",
        "folder/foo.scm",
        "selected.scm",
        "builtin.scm",
        "wasm.scm",
    ] {
        std::fs::write(managed.join(name), "(define valid 1)").unwrap();
        std::fs::write(root.join(name), "(define invalid").unwrap();
    }
    std::fs::write(managed.join("fallback.scm"), "(define valid 1)").unwrap();
    std::fs::write(root.join("config.kdl"), format!("user_plugin_dir \"{}\"\nplugins {{ fallback location=\"file:fallback.scm\"; selected location=\"file:{}\"; builtin location=\"yinz:tab-bar\"; wasm location=\"file:missing.wasm\"; }}\n", managed.display(), root.join("selected.scm").display())).unwrap();
    for name in ["foo", "foo.scm", "folder/foo", "fallback"] {
        let report = success(
            command(root)
                .args(["--config", "config.kdl", "plugins", "check", name, "--json"])
                .output()
                .unwrap(),
        );
        assert!(report["diagnostics"].as_array().unwrap().is_empty());
    }
    let out = command(root)
        .args([
            "--config",
            "config.kdl",
            "plugins",
            "check",
            "selected",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!report["diagnostics"].as_array().unwrap().is_empty());
    for alias in ["builtin", "wasm"] {
        failure(
            command(root)
                .args(["--config", "config.kdl", "plugins", "check", alias])
                .output()
                .unwrap(),
            "unsupported",
        );
    }
    success(
        command(root)
            .args([
                "--config",
                "config.kdl",
                "plugins",
                "check",
                "./managed/selected.scm",
                "--json",
            ])
            .output()
            .unwrap(),
    );
}

#[test]
fn plugin_reload_explicit_targets_preserve_instance() {
    let session = Session::with_setup(|dir| {
        let path = dir.join("space #?%23 name.scm");
        std::fs::write(&path, r#"(define (render rows columns) "VERSION_0")"#).unwrap();
        std::fs::write(dir.join("config.kdl"), format!("load_plugins clear-defaults=true {{}}\ndetachable_panes false\nplugins {{ selected location=\"{}\" {{ greeting \"CONFIGURED\"; }}; }}\n", url::Url::from_file_path(&path).unwrap())).unwrap();
        std::fs::write(
            dir.join("layout.kdl"),
            "layout { pane { plugin location=\"selected\"; }; pane command=\"/bin/sh\"; }\n",
        )
        .unwrap();
    });
    let path = session.0.path().join("space #?%23 name.scm");
    let pane = session.plugin_pane("selected");
    session.rendered(&pane, "VERSION_0");
    let targets = [
        "selected".into(),
        path.display().to_string(),
        "./space #?%23 name.scm".into(),
        format!(
            "../{}/space #?%23 name.scm",
            session.0.path().file_name().unwrap().to_str().unwrap()
        ),
        format!("file:{}", url::Url::from_file_path(&path).unwrap().path()),
        url::Url::from_file_path(&path).unwrap().to_string(),
        "file:./space #?%23 name.scm".into(),
    ];
    for (index, target) in targets.iter().enumerate() {
        let content = format!("VERSION_{}", index + 1);
        std::fs::write(&path, format!(r#"(define greeting "") (define (load config) (set! greeting (hash-ref config "greeting"))) (define (render rows columns) (string-append "{content} " greeting))"#)).unwrap();
        session.plugin_cli(&["plugins", "reload", target]);
        session.rendered(&pane, &format!("{content} CONFIGURED"));
        assert_eq!(session.plugin_pane("selected"), pane);
    }
}

#[test]
fn plugin_logs_use_resolved_managed_source_after_deletion() {
    let session = Session::with_setup(|dir| {
        let managed = dir.join("managed #%");
        std::fs::create_dir(&managed).unwrap();
        std::fs::write(
            managed.join("keys.scm"),
            r#"(define (load config) (yinz-log "MANAGED_KEYS_LOADED"))"#,
        )
        .unwrap();
        let config = std::fs::read_to_string(dir.join("config.kdl")).unwrap();
        std::fs::write(
            dir.join("config.kdl"),
            format!(
                "user_plugin_dir \"{}\"\n{}",
                managed.display(),
                config
                    .replace(
                        "plugins {",
                        "plugins { selected location=\"file:keys.scm\";"
                    )
                    .replace(
                        "load_plugins clear-defaults=true {",
                        "load_plugins clear-defaults=true { selected;"
                    )
            ),
        )
        .unwrap();
    });
    let source = session.0.path().join("managed #%/keys.scm");
    let location = url::Url::from_file_path(source.canonicalize().unwrap())
        .unwrap()
        .to_string();
    let status = success(session.run(&["plugins", "list", "--running", "--json"], None));
    let plugin = status["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plugin| plugin["location"].as_str().unwrap().ends_with("keys.scm"))
        .unwrap();
    assert_eq!(plugin["location"], location);
    let id = format!("plugin_{}", plugin["id"].as_u64().unwrap());
    let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
    loop {
        let log = success(session.run(&["plugins", "log", &id, "--json"], None));
        if log["output"]
            .as_str()
            .unwrap()
            .contains("MANAGED_KEYS_LOADED")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "plugin did not log its load: {log}"
        );
    }
    for deleted in [false, true] {
        if deleted {
            std::fs::remove_file(&source).unwrap();
        }
        for target in ["keys", "selected", id.as_str()] {
            let log = success(session.run(&["plugins", "log", target, "--json"], None));
            assert!(log["runtime"].as_str().unwrap().starts_with(&id), "{log}");
            assert!(
                log["output"]
                    .as_str()
                    .unwrap()
                    .contains("MANAGED_KEYS_LOADED"),
                "{log}"
            );
        }
        let status = success(session.run(&["plugins", "list", "--running", "--json"], None));
        let current = status["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|current| current["id"] == plugin["id"])
            .unwrap();
        assert_eq!(current["location"], location);
    }
}

#[test]
fn plugin_reload_headless_reserved_paths_preserve_instance() {
    fn write_source(path: &Path, version: usize) {
        std::fs::write(path, format!(r#"(define (load config) (yinz-log (string-append "RELOAD_STATE " (yinz-json-string (hash "version" {version} "id" (hash-ref (yinz-command "GetPluginIds") "plugin_id") "greeting" (if (hash-contains? config "greeting") (hash-ref config "greeting") ""))))))"#)).unwrap();
    }
    fn loaded(session: &Session, target: &str, version: usize) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let log = session.plugin_cli(&["plugins", "log", target, "--lines", "100"]);
            for line in log.lines() {
                if let Some((_, state)) = line.split_once("RELOAD_STATE ") {
                    let state: Value = serde_json::from_str(state).unwrap();
                    if state["version"] == version {
                        return state;
                    }
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "plugin version {version} did not load: {log}"
            );
        }
    }
    let session = Session::with_setup(|dir| {
        std::fs::create_dir(dir.join("plugins")).unwrap();
        std::os::unix::fs::symlink(dir.join("plugins"), dir.join("linked")).unwrap();
        let path = dir.join("linked/space #?%23 name.scm");
        write_source(&path, 0);
        let config = std::fs::read_to_string(dir.join("config.kdl")).unwrap();
        std::fs::write(
            dir.join("config.kdl"),
            config
                .replace(
                    "plugins {",
                    &format!(
                        "plugins {{ selected location=\"{}\" {{ greeting \"CONFIGURED\"; }};",
                        url::Url::from_file_path(&path).unwrap()
                    ),
                )
                .replace(
                    "load_plugins clear-defaults=true {",
                    "load_plugins clear-defaults=true { selected;",
                ),
        )
        .unwrap();
    });
    let path = session.0.path().join("linked/space #?%23 name.scm");
    let canonical = path.canonicalize().unwrap();
    let initial = loaded(&session, "selected", 0);
    let initial_panes = success(session.run(&["action", "list-panes", "--json"], None));
    assert_eq!(initial["greeting"], "CONFIGURED");
    let targets = [
        path.display().to_string(),
        canonical.display().to_string(),
        "./linked/space #?%23 name.scm".into(),
        format!("file:{}", url::Url::from_file_path(&path).unwrap().path()),
        url::Url::from_file_path(&canonical).unwrap().to_string(),
        "selected".into(),
    ];
    for (index, target) in targets.iter().enumerate() {
        write_source(&path, index + 1);
        let reply =
            success(session.run(&["plugins", "reload", target, "--headless", "--json"], None));
        assert_eq!(reply["plugins"][0]["id"], initial["id"], "{reply}");
        let state = loaded(&session, "selected", index + 1);
        assert_eq!(state["id"], initial["id"]);
        assert_eq!(state["greeting"], "CONFIGURED");
    }
    // Missing reloads fail. Explicit start supplies the new background identity.
    let missing = session.0.path().join("new.scm");
    write_source(&missing, 100);
    let target = missing.to_str().unwrap();
    let missing_reply = session.run(&["plugins", "reload", target, "--headless", "--json"], None);
    assert!(!missing_reply.status.success());
    let missing_reply: Value = serde_json::from_slice(&missing_reply.stdout).unwrap();
    assert!(missing_reply["error"]
        .as_str()
        .unwrap()
        .contains("not loaded; use plugins start"));
    let reply = success(session.run(&["plugins", "start", target, "--json"], None));
    assert!(reply["plugins"][0]["background"].as_bool().unwrap());
    let original = loaded(&session, target, 100);
    write_source(&missing, 101);
    let reply = success(session.run(&["plugins", "reload", target, "--headless", "--json"], None));
    assert_eq!(reply["plugins"][0]["id"], original["id"]);
    assert_eq!(loaded(&session, target, 101)["id"], original["id"]);
    let panes = success(session.run(&["action", "list-panes", "--json"], None));
    assert_eq!(
        panes.as_array().unwrap().len(),
        initial_panes.as_array().unwrap().len() + 1
    );
    assert!(
        panes
            .as_array()
            .unwrap()
            .iter()
            .filter(|pane| pane["is_plugin"] == true)
            .all(|pane| pane["is_suppressed"] == true),
        "headless reload opened a visible pane: {panes}"
    );
    let duplicate = success(session.run(
        &[
            "plugins",
            "start",
            canonical.to_str().unwrap(),
            "--configuration-json",
            "{}",
            "--json",
        ],
        None,
    ));
    let ambiguous = session.run(
        &["plugins", "log", canonical.to_str().unwrap(), "--json"],
        None,
    );
    assert!(!ambiguous.status.success());
    let ambiguous: Value = serde_json::from_slice(&ambiguous.stdout).unwrap();
    assert!(ambiguous["error"].as_str().unwrap().contains("ambiguous"));
    let duplicate_id = format!("plugin_{}", duplicate["plugins"][0]["id"].as_u64().unwrap());
    let stopped = success(session.run(&["plugins", "stop", &duplicate_id, "--json"], None));
    assert_eq!(stopped["plugins"][0]["state"], "stopped");
    let builtin = success(session.run(&["plugins", "log", "yinz:agent-opencode", "--json"], None));
    assert!(builtin["runtime"].as_str().unwrap().starts_with("plugin_"));
}

#[path = "agent_theme_launch.rs"]
mod agent_theme_launch;

#[test]
fn attached_client_capture_keeps_only_the_byte_tail() {
    let output = Mutex::new(Vec::new());
    let mut input = vec![b'x'; ATTACH_OUTPUT_LIMIT + 1024];
    input.extend_from_slice(b"last bytes");
    capture_tail(input.as_slice(), &output);
    assert_eq!(
        *output.lock().unwrap(),
        input[input.len() - ATTACH_OUTPUT_LIMIT..]
    );
}

#[test]
fn attached_client_capture_retains_early_exit_output() {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "printf 'attachment failed'; exit 23"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let captured = output.clone();
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || capture_tail(stdout, &captured));
    let mut client = AttachedClient(child, Some(reader), output);
    assert_eq!(client.0.wait().unwrap().code(), Some(23));
    client.1.take().unwrap().join().unwrap();
    assert_eq!(*client.2.lock().unwrap(), b"attachment failed");
}

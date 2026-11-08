//! Real binary and real OS process checks for launch admission and idempotency.
use super::*;
use serde_json::{json, Value};

fn session(limit: Option<usize>) -> Session {
    let session = Session::new();
    yinz_steel_builtins::install(
        &yinz_steel_builtins::asset_dir(),
        &session.0.path().join("data/yinz"),
    )
    .unwrap();
    let mut args = vec![
        "--layout-string",
        "layout { pane command=\"/bin/cat\"; }",
        "attach",
        "--create-background",
        NAME,
    ];
    let limit = limit.map(|value| value.to_string());
    if let Some(limit) = &limit {
        args.splice(0..0, ["--max-panes", limit]);
    }
    session.success(&args);
    session.wait_for_panes(false);
    session
}
fn support(session: &Session) -> String {
    let output = session.success(&["--session", NAME, "agent", "launch-support", "--json"]);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["output"], "agent-launch-transaction-v2");
    value["server_instance"].as_str().unwrap().to_owned()
}
fn spawn(session: &Session, launch_id: &str, file: &std::path::Path, extra: &[&str]) -> Value {
    let mut args = vec![
        "--session",
        NAME,
        "agent",
        "spawn",
        "--launch-id",
        launch_id,
        "--file",
        file.to_str().unwrap(),
        "--stack-for",
        "terminal_0",
        "--allow-outside-worktree",
        "--json",
    ];
    args.extend(extra);
    let output = session.run(&args);
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {output:?}"))
}
fn provider(session: &Session) -> (PathBuf, PathBuf) {
    let marker = session.0.path().join("provider-executed");
    let invoked = session.0.path().join("provider-invoked");
    let provider = session.0.path().join("claude");
    yinz_test_support::write_warm_script(&provider, &format!("printf 'invoked\\n' >> '{}'\n[ \"$1\" = --version ] && {{ echo '2.1.296 (Claude Code)'; exit 0; }}\nprintf 'executed\\n' >> '{}'\nexec /bin/cat", invoked.display(), marker.display()));
    let file = session.0.path().join("launch.json");
    fs::write(
        &file,
        json!({"kind":"claude", "command":provider, "cwd":session.0.path().join("home")})
            .to_string(),
    )
    .unwrap();
    (file, marker)
}

#[test]
fn launch_transaction_real_capacity_rejection_has_zero_execs_and_no_checkout() {
    let session = session(Some(1));
    let (file, marker) = provider(&session);
    let id = format!("{}.capacity", support(&session));
    let before = session.list_panes().unwrap();
    let result = spawn(&session, &id, &file, &["--branch", "must-not-be-created"]);
    assert_eq!(result["launch"]["outcome"], "rejected", "{result}");
    assert_eq!(result["launch"]["reason"], "pane_limit", "{result}");
    assert_eq!(result["launch"]["process"], "not_started");
    assert_eq!(result["launch"]["resources"]["cleanup_pending"], false);
    assert!(!marker.exists());
    assert!(!session.0.path().join("provider-invoked").exists());
    assert!(!session.0.path().join("home/.worktrees").exists());
    assert_eq!(session.list_panes().unwrap(), before);
    let repeated = spawn(&session, &id, &file, &["--branch", "must-not-be-created"]);
    assert_eq!(repeated["launch"]["outcome"], "rejected");
    assert!(!marker.exists());
}

#[test]
fn launch_transaction_real_repeated_identity_runs_one_provider_and_closed_status_stays_closed() {
    let session = session(None);
    let (file, marker) = provider(&session);
    let id = format!("{}.once", support(&session));
    let first = spawn(&session, &id, &file, &[]);
    assert_eq!(first["launch"]["outcome"], "started", "{first}");
    let second = spawn(&session, &id, &file, &[]);
    assert_eq!(first["spawned"], second["spawned"]);
    // The provider writes before blocking in cat. Read through the pane's output barrier,
    // then the marker counts real executions, independently of the fake OS harness.
    let observed = yinz_test_support::Watched::new(None);
    std::thread::scope(|scope| {
        let observed = &observed;
        let marker = &marker;
        scope.spawn(move || {
            let end = std::time::Instant::now() + Duration::from_secs(30);
            while std::time::Instant::now() < end {
                if let Ok(value) = fs::read_to_string(marker) {
                    *observed.lock() = Some(value);
                    return;
                }
                std::thread::yield_now();
            }
        });
        observed.wait_until("provider marker", Duration::from_secs(30), |value| {
            value.clone()
        });
    });
    assert_eq!(fs::read_to_string(&marker).unwrap(), "executed\n");
    let conflict = spawn(&session, &id, &file, &["--model", "different"]);
    assert_eq!(conflict["launch"]["outcome"], "conflict");
    assert_eq!(fs::read_to_string(&marker).unwrap(), "executed\n");
    let pane = first["spawned"]["pane_id"].as_str().unwrap();
    session.success(&["--session", NAME, "action", "close-pane", "--pane-id", pane]);
    let output = session.run(&["--session", NAME, "agent", "launch-status", &id, "--json"]);
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["launch"]["outcome"], "started");
    assert_eq!(status["launch"]["disposition"], "closed", "{status}");
}

#[test]
fn launch_transaction_real_prepare_and_exec_failure_release_only_the_owned_pane() {
    let session = session(None);
    let (file, marker) = provider(&session);
    let server = support(&session);
    let before = session.list_panes().unwrap();
    let preparation = spawn(
        &session,
        &format!("{server}.git-failure"),
        &file,
        &["--branch", "failure", "--base", "absent-base"],
    );
    assert_eq!(preparation["launch"]["outcome"], "failed", "{preparation}");
    assert_eq!(preparation["launch"]["reason"], "prepare_failed");
    assert_eq!(preparation["launch"]["process"], "not_started");
    assert_eq!(preparation["launch"]["resources"]["cleanup_pending"], false);
    assert!(!marker.exists());
    assert_eq!(session.list_panes().unwrap(), before);
    fs::write(&file, json!({"kind":"claude", "command":session.0.path().join("missing/claude"), "cwd":session.0.path().join("home")}).to_string()).unwrap();
    let execution = spawn(&session, &format!("{server}.exec-failure"), &file, &[]);
    assert_eq!(execution["launch"]["outcome"], "failed", "{execution}");
    assert_eq!(execution["launch"]["reason"], "exec_failed");
    assert_eq!(execution["launch"]["resources"]["cleanup_pending"], false);
    assert!(!marker.exists());
    assert_eq!(session.list_panes().unwrap(), before);
}

#[test]
fn launch_transaction_real_revocation_during_git_preparation_has_zero_execs_and_retains_checkout() {
    use std::io::Write;
    let session = session(None);
    let root = session.0.path().join("home");
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "user.name=Fixture",
            "commit",
            "--allow-empty",
            "-m",
            "seed",
        ],
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let entered = session.0.path().join("checkout-hook-entered");
    let release = session.0.path().join("release-checkout");
    assert!(Command::new("mkfifo")
        .arg(&release)
        .status()
        .unwrap()
        .success());
    yinz_test_support::write_warm_script(
        &root.join(".git/hooks/post-checkout"),
        &format!(
            "printf ready > '{}'\nIFS= read -r release < '{}'",
            entered.display(),
            release.display()
        ),
    );
    let (file, provider_marker) = provider(&session);
    let id = format!("{}.revoke-during-git", support(&session));
    let entered_state = yinz_test_support::Watched::new(false);
    let result = std::thread::scope(|scope| {
        struct ReleaseHook(fs::File);
        impl Drop for ReleaseHook {
            fn drop(&mut self) {
                let _ = self.0.write_all(b"continue\n");
            }
        }
        // Keep both FIFO endpoints open and release on assertion failure too: scoped thread
        // joins must never wait on a hook after the test's observation has panicked.
        let mut release_gate = ReleaseHook(
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&release)
                .unwrap(),
        );
        let launch = scope.spawn(|| {
            spawn(
                &session,
                &id,
                &file,
                &["--branch", "reserved-worker", "--base", "HEAD"],
            )
        });
        scope.spawn(|| {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            while std::time::Instant::now() < deadline {
                if entered.exists() {
                    *entered_state.lock() = true;
                    return;
                }
                std::thread::yield_now();
            }
        });
        entered_state.wait_until(
            "Git hook after admitted placeholder",
            Duration::from_secs(30),
            |ready| ready.then_some(()),
        );
        let output = session.run(&["--session", NAME, "agent", "launch-status", &id, "--json"]);
        let pending: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(pending["launch"]["phase"], "prepares", "{pending}");
        assert_eq!(pending["launch"]["process"], "not_started");
        let pane = format!(
            "terminal_{}",
            pending["launch"]["resources"]["slot"]["pane_id"]["Terminal"]
                .as_u64()
                .unwrap()
        );
        // Enter normally starts a held command. A managed reservation must keep that
        // ordinary input path behind the same transaction permit while Git is paused.
        session.run(&[
            "--session",
            NAME,
            "action",
            "send-keys",
            "--pane-id",
            &pane,
            "Enter",
        ]);
        assert!(!session.0.path().join("provider-invoked").exists());
        session.success(&[
            "--session",
            NAME,
            "action",
            "close-pane",
            "--pane-id",
            &pane,
        ]);
        // FIFO release orders the completed layout revocation before Git returns to the host.
        release_gate.0.write_all(b"continue\n").unwrap();
        launch.join().unwrap()
    });
    assert_eq!(result["launch"]["outcome"], "cancelled", "{result}");
    assert_eq!(result["launch"]["process"], "not_started", "{result}");
    assert_eq!(
        result["launch"]["resources"]["cleanup_pending"], false,
        "{result}"
    );
    let checkout = PathBuf::from(
        result["launch"]["resources"]["checkout"]["cwd"]
            .as_str()
            .unwrap(),
    );
    assert!(
        checkout.join(".git").exists(),
        "retained checkout: {result}"
    );
    assert!(result["launch"]["resources"]["git"]
        .as_array()
        .unwrap()
        .iter()
        .any(|resource| resource["ownership"] == "created" && resource["kind"] == "worktree"));
    assert!(!provider_marker.exists());
    assert!(!session.0.path().join("provider-invoked").exists());
    let repeated = spawn(
        &session,
        &id,
        &file,
        &["--branch", "reserved-worker", "--base", "HEAD"],
    );
    assert_eq!(repeated["launch"]["outcome"], "cancelled");
    assert!(!provider_marker.exists());
}

#[test]
fn launch_transaction_real_explicit_suppression_remains_supported() {
    let session = session(None);
    let (file, _) = provider(&session);
    let id = format!("{}.suppressed", support(&session));
    let out = session.run(&[
        "--session",
        NAME,
        "agent",
        "spawn",
        "--launch-id",
        &id,
        "--file",
        file.to_str().unwrap(),
        "--suppressed",
        "--allow-outside-worktree",
        "--json",
    ]);
    let response: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(response["launch"]["outcome"], "started", "{response}");
    let pane: u64 = response["spawned"]["pane_id"]
        .as_str()
        .unwrap()
        .trim_start_matches("terminal_")
        .parse()
        .unwrap();
    let panes = session.list_panes().unwrap();
    assert!(
        panes
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == pane && p["is_suppressed"] == true),
        "{panes}"
    );
}

//! Process-level acceptance: stdin, JSON, live background services, and installation.
use serde_json::{json, Value};
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
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
    cmd.current_dir(dir)
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
struct Session(tempfile::TempDir, String);
impl Session {
    fn new() -> Self {
        let mut session = Self(tempfile::tempdir().unwrap(), String::new());
        let dir = session.0.path();
        assert!(Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg(dir)
            .status()
            .unwrap()
            .success());
        std::fs::write(dir.join("config.kdl"), "plugins { hocus-store location=\"yinz:hocus-store\"; hocus-agentd location=\"yinz:hocus-agentd\"; swamp-bridge location=\"yinz:swamp-bridge\"; }\nload_plugins clear-defaults=true { \"hocus-store\"; \"hocus-agentd\"; \"swamp-bridge\"; }\ndetachable_panes false\n").unwrap();
        std::fs::write(dir.join("layout.kdl"), "layout { pane command=\"/bin/sh\" { args \"-c\" \"printf 'PORTABLE READY\\n'; exec /bin/sh\"; }; }\n").unwrap();
        let out = command(dir)
            .args([
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
        session
    }
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = command(self.0.path());
        cmd.args(["--session", "portable-test"]).args(args);
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
            &[
                "data",
                "fetch",
                "--namespace",
                "shared:hocus",
                "--file",
                "-",
            ],
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
    }
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
            &[
                "data",
                "apply",
                "--namespace",
                "shared:hocus",
                "--file",
                "-",
            ],
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
    success(session.run(
        &["action", "project-config-write", "work", "--file", "-"],
        Some("{\"source\":\"agent_exit { default_branch \\\"main\\\"; }\\n\",\"expected\":null}"),
    ));
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
    assert_eq!(listed["projects"][0]["name"], "renamed");
    assert!(!listed["live"].is_null());
    let catalog = success(session.run(&["data", "catalog"], None));
    assert!(catalog
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity["namespace"] == "plugin:yinz:swamp-bridge"));
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
        .any(|pane| pane["plugin_url"] == "hocus"));

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
            &["action", "start-or-reload-plugin", "yinz:hocus-agentd"],
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
    let source = dir.path().join("source.ts");
    std::fs::write(&source, "import { definePlugin } from 'yinz:plugin'; export default definePlugin({render() { return 'FIRST'; }});").unwrap();
    let out = command(dir.path())
        .args(["plugins", "write", "fixture.ts", "--file"])
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
        command(dir.path())
            .args(["plugins", "edit", "fixture.ts", "--file"])
            .arg(&edits)
            .output()
            .unwrap()
    };
    std::fs::write(&edits, r#"[{"old_string":"FIRST","new_string":"SECOND"}]"#).unwrap();
    success(edit());
    let saved = std::fs::read_to_string(&installed).unwrap();
    assert!(saved.contains("SECOND"));
    assert!(installed.with_file_name("fixture.bak.ts").is_file());
    std::fs::write(&edits, r#"[{"old_string":"SECOND","new_string":"THIRD"},{"old_string":"MISSING","new_string":"bad"}]"#).unwrap();
    assert!(!edit().status.success());
    assert_eq!(std::fs::read_to_string(&installed).unwrap(), saved);
    std::fs::write(installed.with_file_name("fixture.test.ts"), "import { test, loadPlugin, assertEquals } from 'yinz:testing'; test('renders edited source', async () => { const p = await loadPlugin('./fixture.ts'); assertEquals(await p.render(), 'SECOND'); });").unwrap();
    let report = success(
        command(dir.path())
            .args(["plugins", "test", "fixture.ts", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["tests"][0]["status"], "passed");
}

#[test]
fn bundle_installation_preserves_unrelated_files_and_preflights_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("custom agent/skills");
    let install = |args: &[&str]| {
        command(dir.path())
            .args(["skills", "install", "--dest"])
            .arg(&dest)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(install(&["--dry-run"]).status.success());
    assert!(!dest.exists());
    assert!(install(&[]).status.success());
    let names = success(
        command(dir.path())
            .args(["skills", "list"])
            .output()
            .unwrap(),
    );
    assert_eq!(names.as_array().unwrap().len(), 6);
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

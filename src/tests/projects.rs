//! Exercise the public commands as processes with redirected stdin and a real session.
use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Output, Stdio},
};

fn command(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_yinz"));
    cmd.arg("--data-dir")
        .arg(dir.join("yinz"))
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir)
        .env("XDG_DATA_HOME", dir)
        .env("XDG_CACHE_HOME", dir)
        .env("YINZ_SOCKET_DIR", dir)
        .env("TMPDIR", dir)
        .env_remove("YINZ")
        .env_remove("YINZ_SESSION_NAME")
        .env_remove("YINZ_PANE_ID")
        .env_remove("YINZ_PANE_REF")
        .env("SHELL", "/bin/sh")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("one JSON value on stdout")
}

struct Session(tempfile::TempDir);
impl Session {
    fn action(&self, args: &[&str]) -> Output {
        command(self.0.path())
            .args(["--session", "projects-test", "action"])
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = command(self.0.path())
            .args(["kill-session", "projects-test"])
            .output();
    }
}

#[test]
fn projects_commands_return_json_and_failures_with_redirected_stdin() {
    let session = Session(tempfile::tempdir().unwrap());
    let dir = session.0.path();
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &dir.join("yinz")).unwrap();
    let config = dir.join("config.kdl");
    let layout = dir.join("layout.kdl");
    std::fs::write(&config, "plugins { project-store location=\"yinz:project-store\" { shared true; }; }\nload_plugins clear-defaults=true { \"project-store\"; }\ndetachable_panes false\n").unwrap();
    std::fs::write(&layout, "layout { pane command=\"/bin/sh\"; }\n").unwrap();
    let started = command(dir)
        .arg("--config")
        .arg(&config)
        .arg("--layout")
        .arg(&layout)
        .args(["attach", "--create-background", "projects-test"])
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    success(session.action(&["project-remember", "work", "--root", "/work"]));
    let list = success(session.action(&["project-list"]));
    assert_eq!(list["projects"][0]["name"], "work");
    success(session.action(&["project-rename", "work", "renamed"]));
    let list = success(session.action(&["project-list"]));
    assert_eq!(list["projects"][0]["name"], "renamed");
    let error = session.action(&["project-forget", "missing"]);
    assert!(!error.status.success());
    assert!(error.stdout.is_empty());
    assert!(String::from_utf8_lossy(&error.stderr).contains("unknown project"));
    success(session.action(&["project-forget", "renamed"]));
    assert_eq!(
        success(session.action(&["project-list"]))["projects"],
        serde_json::json!([])
    );
}

#[test]
fn projects_skill_install_uses_custom_codex_home_and_defaults_to_both_agents() {
    let temp = tempfile::tempdir().unwrap();
    let codex_home = temp.path().join("custom codex");
    let install = |args: &[&str]| {
        command(temp.path())
            .env("CODEX_HOME", &codex_home)
            .args(["skills", "install", "yinz-sessions"])
            .args(args)
            .output()
            .unwrap()
    };
    let dry = install(&["--dry-run"]);
    assert!(dry.status.success());
    assert!(!codex_home.exists());
    assert!(install(&[]).status.success());
    let codex = codex_home.join("skills/yinz-sessions/SKILL.md");
    let claude = temp.path().join(".claude/skills/yinz-sessions/SKILL.md");
    assert_eq!(
        std::fs::read(&codex).unwrap(),
        std::fs::read(&claude).unwrap()
    );
    assert!(install(&[]).status.success());
    std::fs::write(&codex, "custom").unwrap();
    assert!(!install(&[]).status.success());
    assert_eq!(std::fs::read_to_string(&codex).unwrap(), "custom");
    assert!(install(&["--force"]).status.success());
    assert_eq!(
        std::fs::read(codex).unwrap(),
        std::fs::read(claude).unwrap()
    );
}

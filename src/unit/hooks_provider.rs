//! Provider hooks use the same CLI path for builtin aliases and arbitrary Scheme sources.
use std::{
    path::Path,
    process::{Command, Output},
};

fn hooks(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yinz"))
        .arg("--data-dir")
        .arg(root.join("data"))
        .arg("hooks")
        .args(args)
        .current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env("CODEX_HOME", root.join("codex"))
        .env("PI_CODING_AGENT_DIR", root.join("pi"))
        .env("TMPDIR", root)
        .env_remove("YINZ_COMPILATION_WORKER")
        .env_remove("YINZ_PANE_REF")
        .env_remove("YINZ_REMOTE_PANE_TOKEN")
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn provider(path: &Path, hook_path: &str, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!(
        "(define agent-provider (hash \"kind\" \"fixture-hooks\" \"executable\" \"/usr/bin/true\" \"hooks\" (list (hash \"path\" {hook_path:?} \"content\" {content:?} \"marker\" \"fixture-hook\"))))"
    )).unwrap();
}

#[test]
fn hooks_provider_custom_source_installs_checks_and_removes_owned_hooks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("custom.scm");
    provider(
        &source,
        "fixture/hook.sh",
        "# fixture-hook\n__YINZ_SHELL__ --version\n",
    );
    let source = source.to_str().unwrap();
    let path = root.join("config/fixture/hook.sh");

    assert!(success(hooks(root, &["status", "--agent", source])).contains("missing"));
    assert!(
        success(hooks(root, &["install", "--agent", source, "--dry-run"])).contains("would write")
    );
    assert!(!path.exists());
    success(hooks(root, &["install", "--agent", source]));
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains(env!("CARGO_BIN_EXE_yinz")));
    assert!(!content.contains("__YINZ_SHELL__"));
    success(hooks(root, &["install", "--agent", source]));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    assert_eq!(
        std::fs::read_to_string(path.with_extension("sh.bak")).unwrap(),
        content
    );
    assert!(success(hooks(root, &["status", "--agent", source])).contains("current"));
    std::fs::write(&path, "# fixture-hook old\n").unwrap();
    assert!(success(hooks(root, &["status", "--agent", source])).contains("outdated"));
    success(hooks(root, &["uninstall", "--agent", source]));
    assert!(!path.exists());

    std::fs::write(&path, "# somebody else's hook\n").unwrap();
    for action in ["install", "uninstall", "status"] {
        let output = hooks(root, &[action, "--agent", source]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("was not written by this agent provider"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# somebody else's hook\n"
        );
    }
}

#[test]
fn hooks_provider_aliases_and_all_commands_use_selected_installed_source() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("data/plugins/builtins/agent-opencode.scm");
    provider(&source, "selected/hook", "fixture-hook selected source\n");
    let path = root.join("config/selected/hook");
    for name in ["opencode", "OpenCode", "OPENCODE", "yinz:agent-opencode"] {
        success(hooks(root, &["install", "--agent", name]));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fixture-hook selected source\n"
        );
        assert!(success(hooks(root, &["status", "--agent", name])).contains("current"));
        success(hooks(root, &["uninstall", "--agent", name]));
        assert!(!path.exists());
    }
    for selection in [&[][..], &["--agent", "all"][..]] {
        let mut args = vec!["install"];
        args.extend(selection);
        success(hooks(root, &args));
        assert!(path.exists());
        args[0] = "status";
        let output = success(hooks(root, &args));
        assert!(output.contains("selected/hook (current)"), "{output}");
        assert_eq!(output.matches("pane:").count(), 1);
        args[0] = "uninstall";
        success(hooks(root, &args));
        assert!(!path.exists());
    }
}

#[test]
fn hooks_provider_project_rejection_and_all_failure_isolation_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("custom.scm");
    provider(&source, "fixture/hook", "fixture-hook\n");
    for agent in [source.to_str().unwrap(), "opencode", "OpenCode"] {
        for flag in ["--project", "--project-dir"] {
            for action in ["install", "uninstall"] {
                let mut args = vec![action, "--agent", agent, flag];
                if flag == "--project-dir" {
                    args.push(root.to_str().unwrap());
                }
                let output = hooks(root, &args);
                assert!(!output.status.success());
                assert!(String::from_utf8_lossy(&output.stderr)
                    .contains("declared home or config paths"));
            }
        }
    }
    assert!(!root.join("config").exists());
    provider(
        &root.join("data/plugins/builtins/agent-opencode.scm"),
        "fixture/hook",
        "fixture-hook\n",
    );
    std::fs::create_dir_all(root.join("claude")).unwrap();
    std::fs::write(root.join("claude/settings.json"), "invalid json").unwrap();
    let output = hooks(root, &["install", "--agent", "all"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Claude Code"));
    assert!(root.join("config/fixture/hook").exists());
    assert!(root.join("codex/hooks.json").exists());
    assert!(root
        .join("pi/extensions")
        .join(yinz_types::agent::PI_EXTENSION_FILE)
        .exists());
    assert_eq!(
        std::fs::read_to_string(root.join("claude/settings.json")).unwrap(),
        "invalid json"
    );
    // Native status reports malformed settings as text; provider ownership errors fail.
    std::fs::write(root.join("config/fixture/hook"), "foreign hook").unwrap();
    let output = hooks(root, &["status", "--agent", "all"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("was not written by this agent provider")
    );
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("pi:"), "{output}");
    assert!(
        output.contains("yinz-agent-report.ts (current)"),
        "{output}"
    );
    assert_eq!(output.matches("pane:").count(), 1);
}

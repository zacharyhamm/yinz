use super::plugin_log_lines;
use crate::{background_worker::Workers, steel::SteelPlugin, test_env::test_env};
use std::sync::OnceLock;
use yinz_test_support::Watched;
use yinz_types::input::{
    git_plugin::GitPlugin,
    layout::{RunPlugin, RunPluginLocation},
    plugins::PluginAliases,
};

struct Capture(Watched<Vec<String>>);
impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        self.0.lock().push(record.args().to_string());
    }
    fn flush(&self) {}
}

#[test]
fn connected_repl_separates_stdout_and_queryable_diagnostics_on_success_and_failure() {
    use serde_json::{json, Value};
    use yinz_bus::{channels, ServerInstruction};
    use yinz_runtime::channels::SenderWithContext;
    use yinz_types::data::{PipeMessage, PipeSource, PluginTag};

    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    let capture = CAPTURE.get_or_init(|| Capture(Watched::new(Vec::new())));
    log::set_logger(capture).unwrap();
    log::set_max_level(log::LevelFilter::Debug);
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let mut env = test_env();
    env.plugin_cwd = cwd.clone();
    env.plugin_config = yinz_config::plugins::PluginConfig::from_run_plugin(&RunPlugin {
        location: RunPluginLocation::Yinz(PluginTag::new("repl")),
        ..Default::default()
    })
    .unwrap();
    let identity = env.log_identity();
    let (sender, receiver) = channels::unbounded();
    env.senders.to_server = Some(SenderWithContext::new(sender));
    let mut plugin =
        SteelPlugin::new("", std::path::Path::new("repl"), env, Default::default()).unwrap();
    let cases = [
        (
            r#"(display "visible") (yinz-log "success diagnostic") 42"#,
            "visible",
            "success diagnostic",
            None,
        ),
        (
            r#"(display "before failure") (yinz-log "eval diagnostic") (error "eval failed")"#,
            "before failure",
            "eval diagnostic",
            Some("eval failed"),
        ),
        (
            r#"(display "immediate") (yinz-then 0 (lambda (_) (display " queued") (yinz-log "queued diagnostic"))) (void)"#,
            "immediate queued",
            "queued diagnostic",
            None,
        ),
        (
            r#"(%async-enqueue %async-client (lambda () (display "poll output") (yinz-log "poll diagnostic") (error "poll failed")))"#,
            "poll output",
            "poll diagnostic",
            Some("poll failed"),
        ),
    ];
    for (source, stdout, diagnostic, error) in cases {
        plugin
            .pipe(&PipeMessage::new(
                PipeSource::Cli("repl-logs".into()),
                "__yinz_repl_eval",
                &Some(source.into()),
                &None,
                true,
            ))
            .unwrap();
        let (instruction, _) = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let ServerInstruction::CliPipeOutput(pipe, output) = instruction else {
            panic!("unexpected REPL response: {instruction:?}");
        };
        assert_eq!(pipe, "repl-logs");
        let reply: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(reply["output"], stdout);
        if let Some(error) = error {
            assert!(reply["error"].as_str().unwrap().contains(error), "{reply}");
        } else {
            assert!(reply.get("error").is_none(), "{reply}");
        }
        if diagnostic == "success diagnostic" {
            assert_eq!(reply["values"], json!(["42"]));
        }
        assert!(capture
            .0
            .get()
            .iter()
            .any(|line| line.contains(&identity) && line.ends_with(diagnostic)));
    }
    let log_dir = cwd.join(format!("yinz-{}/yinz-log", nix::unistd::Uid::current()));
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_file = log_dir.join("yinz.log");
    std::fs::write(
        &log_file,
        capture.0.get().join("\n") + "\nunrelated diagnostic\n",
    )
    .unwrap();
    let env = test_env();
    let api = plugin_log_lines(&env, "yinz:repl", 100, &log_file).unwrap();
    for (_, stdout, diagnostic, _) in cases {
        assert!(api.contains(diagnostic), "{api}");
        assert!(!api.contains(stdout), "{api}");
    }
    assert!(!api.contains("unrelated"));
    let config = cwd.join("config.kdl");
    std::fs::write(&config, "").unwrap();
    let output = std::process::Command::new(std::env::var_os("YINZ_COMPILATION_WORKER").unwrap())
        .current_dir(&cwd)
        .env("TMPDIR", &cwd)
        .env("YINZ_SOCKET_DIR", cwd.join("no-sessions"))
        .env_remove("YINZ_SESSION_NAME")
        .args([
            "--config",
            config.to_str().unwrap(),
            "plugins",
            "log",
            "yinz:repl",
            "--lines",
            "100",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["output"], api);
    assert_eq!(reply["historical"], true);
}

#[test]
fn external_plugin_logs_are_isolated_through_cli_and_host() {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    let capture = CAPTURE.get_or_init(|| Capture(Watched::new(Vec::new())));
    log::set_logger(capture).unwrap();
    log::set_max_level(log::LevelFilter::Debug);
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let mut aliases = PluginAliases::default();
    for label in ["first", "second"] {
        let folder = cwd.join(label);
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("main.scm");
        let source = format!(
            r#"
(yinz-log "{label} console")
(define (worker-failure event) (error "{label} worker exception"))
(yinz-register-workers (hash "task" (lambda (name payload)
  (yinz-register-worker-events worker-failure)
  (yinz-log "{label} worker"))))
"#
        );
        std::fs::write(&path, &source).unwrap();
        let location = RunPluginLocation::Git(GitPlugin {
            repo: format!("https://example.com/{label}.git"),
            entrypoint: "main.scm".into(),
            revision: None,
        });
        aliases.aliases.insert(
            label.into(),
            RunPlugin {
                location: location.clone(),
                ..Default::default()
            },
        );
        let mut env = test_env();
        env.plugin_cwd = cwd.clone();
        env.plugin_config.path = path.clone();
        env.plugin_config.location = location;
        env.resolved_source_path = Some(path.clone());
        let identity = env.log_identity();
        log::error!("{} attributed error", env.name());
        let mut worker_env = test_env();
        worker_env.plugin_config = env.plugin_config.clone();
        worker_env.plugin_cwd = env.plugin_cwd.clone();
        worker_env.resolved_source_path = env.resolved_source_path.clone();
        let workers = Workers::new(0);
        workers
            .spawn("task".into(), &source, &path, worker_env)
            .unwrap();
        workers
            .post("task", env.client_id, "job".into(), "".into())
            .unwrap();
        let client = env.client_id;
        let _plugin = SteelPlugin::new(&source, &path, env, Default::default()).unwrap();
        capture.0.wait_until_described(
            "worker log",
            std::time::Duration::from_secs(20),
            |lines| {
                lines
                    .iter()
                    .any(|line| line.ends_with(&format!("{label} worker")))
                    .then_some(())
            },
            |lines| format!("{lines:#?}"),
        );
        workers.permissions(
            client,
            &yinz_types::data::Event::CustomMessage("fail".into(), "".into()),
        );
        capture.0.wait_until_described(
            "worker failure diagnostics",
            std::time::Duration::from_secs(20),
            |lines| {
                [
                    "Plugin worker task failed",
                    &format!("{label} worker exception"),
                    "main.scm:3",
                ]
                .iter()
                .all(|message| {
                    lines
                        .iter()
                        .any(|line| line.contains(&identity) && line.contains(message))
                })
                .then_some(())
            },
            |lines| format!("{lines:#?}"),
        );
        workers.stop();
    }
    let log_dir = cwd.join(format!("yinz-{}/yinz-log", nix::unistd::Uid::current()));
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_file = log_dir.join("yinz.log");
    std::fs::write(
        &log_file,
        capture.0.get().join("\n") + "\nlegacy main.scm\n",
    )
    .unwrap();
    let config = cwd.join("config.kdl");
    std::fs::write(&config, "plugins { first repo=\"https://example.com/first.git\" entrypoint=\"main.scm\"; second repo=\"https://example.com/second.git\" entrypoint=\"main.scm\"; }\n").unwrap();
    let mut env = test_env();
    env.plugin_cwd = cwd.clone();
    *env.plugin_aliases.lock().unwrap() = aliases;
    for label in ["first", "second"] {
        for target in [label.to_owned(), format!("./{label}/main.scm")] {
            let api = plugin_log_lines(&env, &target, 100, &log_file).unwrap();
            for message in ["console", "worker"] {
                assert!(api.contains(&format!("{label} {message}")), "{api}");
            }
            for message in [
                "attributed error",
                "Plugin worker task failed",
                &format!("{label} worker exception"),
                "main.scm:3",
            ] {
                assert!(api.contains(message), "{api}");
            }
            assert!(
                !api.contains(if label == "first" { "second" } else { "first" }),
                "{api}"
            );
            assert!(!api.contains("legacy"));
            let output =
                std::process::Command::new(std::env::var_os("YINZ_COMPILATION_WORKER").unwrap())
                    .current_dir(&cwd)
                    .env("TMPDIR", &cwd)
                    .env("YINZ_SOCKET_DIR", cwd.join("no-sessions"))
                    .env_remove("YINZ_SESSION_NAME")
                    .args([
                        "--config",
                        config.to_str().unwrap(),
                        "plugins",
                        "log",
                        &target,
                        "--lines",
                        "100",
                        "--json",
                    ])
                    .output()
                    .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(reply["output"], api);
            assert_eq!(reply["historical"], true);
            assert!(reply["runtime"].as_str().unwrap().contains("unknown"));
        }
    }
}

#[test]
fn file_alias_fallback_logs_match_through_cli_and_host() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let scripts = cwd.join("scripts");
    let data = cwd.join("data");
    let log_dir = cwd.join(format!("yinz-{}/yinz-log", nix::unistd::Uid::current()));
    for directory in [&scripts, &data.join("plugins"), &log_dir] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let mut aliases = PluginAliases::default();
    let mut entries = Vec::new();
    for (alias, name, path) in [
        ("script", "keys.scm", scripts.join("keys.scm")),
        ("local", "local.scm", cwd.join("local.scm")),
    ] {
        std::fs::write(&path, "source").unwrap();
        let (location, relative_path) =
            yinz_runtime::plugin_location::parse_plugin_location_with_relative(
                &format!("file:{name}"),
                Some(cwd.clone()),
            )
            .unwrap();
        entries.push(format!(
            "{} {alias} log",
            crate::plugin_authoring::LogIdentity::new(&location, &path, &cwd).marker()
        ));
        aliases.aliases.insert(
            alias.into(),
            RunPlugin {
                location,
                relative_path,
                ..Default::default()
            },
        );
    }
    let log_file = log_dir.join("yinz.log");
    std::fs::write(&log_file, entries.join("\n") + "\n").unwrap();
    let config = cwd.join("config.kdl");
    std::fs::write(&config, format!(
        "user_plugin_dir {:?}\nplugins {{ script location=\"file:keys.scm\"; local location=\"file:local.scm\"; }}\n",
        scripts.to_str().unwrap()
    )).unwrap();
    let mut env = test_env();
    env.plugin_cwd = cwd.clone();
    env.user_plugin_dir = Some(scripts);
    *env.plugin_aliases.lock().unwrap() = aliases;
    for (alias, expected) in ["script", "local"].into_iter().zip(entries) {
        let api = plugin_log_lines(&env, alias, 100, &log_file).unwrap();
        assert_eq!(api, expected + "\n");
        let output =
            std::process::Command::new(std::env::var_os("YINZ_COMPILATION_WORKER").unwrap())
                .current_dir(&cwd)
                .env("TMPDIR", &cwd)
                .env("YINZ_SOCKET_DIR", cwd.join("no-sessions"))
                .env_remove("YINZ_SESSION_NAME")
                .arg("--data-dir")
                .arg(&data)
                .arg("--config")
                .arg(&config)
                .args(["plugins", "log", alias, "--json"])
                .output()
                .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["output"], api);
        assert_eq!(reply["historical"], true);
        assert!(reply["runtime"].as_str().unwrap().contains("unknown"));
    }
}

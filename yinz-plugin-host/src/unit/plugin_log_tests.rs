use super::plugin_log_lines;
use crate::{
    deno::{background_worker::Workers, DenoPlugin},
    logging_pipe::LoggingPipe,
    test_env::test_env,
};
use std::{io::Write, sync::OnceLock};
use yinz_test_support::{default_timeout, Watched};
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
        let path = folder.join("main.ts");
        let source = format!("console.log('{label} console'); export default {{ workers: {{ task() {{ yinz.on('CustomMessage', function workerFailure() {{ throw Error('{label} worker exception'); }}); console.log('{label} worker'); }} }} }};");
        std::fs::write(&path, &source).unwrap();
        let location = RunPluginLocation::Git(GitPlugin {
            repo: format!("https://example.com/{label}.git"),
            entrypoint: "main.ts".into(),
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
        let mut pipe = LoggingPipe::new(&identity, env.plugin_id);
        writeln!(pipe, "{label} stderr").unwrap();
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
        let _plugin = DenoPlugin::new(&source, &path, env, Default::default()).unwrap();
        capture
            .0
            .wait_until("worker log", default_timeout(), |lines| {
                lines
                    .iter()
                    .any(|line| line.ends_with(&format!("{label} worker")))
                    .then_some(())
            });
        workers.permissions(
            client,
            &yinz_types::data::Event::CustomMessage("fail".into(), "".into()),
        );
        capture
            .0
            .wait_until("worker failure diagnostics", default_timeout(), |lines| {
                [
                    "Deno worker task failed",
                    &format!("{label} worker exception"),
                    "at workerFailure",
                ]
                .iter()
                .all(|message| {
                    lines
                        .iter()
                        .any(|line| line.contains(&identity) && line.contains(message))
                })
                .then_some(())
            });
        workers.stop();
    }
    let log_dir = cwd.join(format!("yinz-{}/yinz-log", nix::unistd::Uid::current()));
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_file = log_dir.join("yinz.log");
    std::fs::write(&log_file, capture.0.get().join("\n") + "\nlegacy main.ts\n").unwrap();
    let config = cwd.join("config.kdl");
    std::fs::write(&config, "plugins { first repo=\"https://example.com/first.git\" entrypoint=\"main.ts\"; second repo=\"https://example.com/second.git\" entrypoint=\"main.ts\"; }\n").unwrap();
    let mut env = test_env();
    env.plugin_cwd = cwd.clone();
    *env.plugin_aliases.lock().unwrap() = aliases;
    for label in ["first", "second"] {
        for target in [label.to_owned(), format!("./{label}/main.ts")] {
            let api = plugin_log_lines(&env, &target, 100, &log_file).unwrap();
            for message in ["console", "worker", "stderr"] {
                assert!(api.contains(&format!("{label} {message}")), "{api}");
            }
            for message in [
                "attributed error",
                "Deno worker task failed",
                &format!("{label} worker exception"),
                "at workerFailure",
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
                    .args([
                        "--config",
                        config.to_str().unwrap(),
                        "plugins",
                        "log",
                        &target,
                        "--lines",
                        "100",
                    ])
                    .output()
                    .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8(output.stdout).unwrap(), api);
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
        ("script", "keys.ts", scripts.join("keys.ts")),
        ("wasm", "keys", data.join("plugins/keys.wasm")),
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
        "user_plugin_dir {:?}\nplugins {{ script location=\"file:keys.ts\"; wasm location=\"file:keys\"; }}\n",
        scripts.to_str().unwrap()
    )).unwrap();
    let mut env = test_env();
    env.plugin_cwd = cwd.clone();
    env.user_plugin_dir = Some(scripts);
    env.yinz_data_dir = data.clone();
    *env.plugin_aliases.lock().unwrap() = aliases;
    for (alias, expected) in ["script", "wasm"].into_iter().zip(entries) {
        let api = plugin_log_lines(&env, alias, 100, &log_file).unwrap();
        assert_eq!(api, expected + "\n");
        let output =
            std::process::Command::new(std::env::var_os("YINZ_COMPILATION_WORKER").unwrap())
                .current_dir(&cwd)
                .env("TMPDIR", &cwd)
                .arg("--data-dir")
                .arg(&data)
                .arg("--config")
                .arg(&config)
                .args(["plugins", "log", alias])
                .output()
                .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), api);
    }
}

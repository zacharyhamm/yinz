use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

fn registration(root: &Path, pane: &str, launcher_pid: u32) -> Registration {
    Registration::new(
        root,
        HashMap::from([
            ("YINZ_PANE_REF".into(), pane.into()),
            ("YINZ_PANE_ID".into(), "terminal_9".into()),
        ]),
        launcher_pid,
    )
    .unwrap()
}

fn ancestors_of(pid: u32) -> HashSet<u32> {
    HashSet::from([pid, std::process::id()])
}

#[test]
fn registration_binds_before_muse_is_spawned() {
    let root = tempfile::tempdir().unwrap();
    let _live = registration(root.path(), "aaaaaaaaaaaa", std::process::id());
    assert!(bind_session(root.path(), &ancestor_pids(), "ses-early", Some("startup")).unwrap());
    assert_eq!(
        binding_for(root.path(), "ses-early").unwrap().unwrap().env["YINZ_PANE_REF"],
        "aaaaaaaaaaaa"
    );
}

#[test]
fn sessions_bind_by_ancestry_and_route_by_session() {
    let root = tempfile::tempdir().unwrap();
    let first = registration(root.path(), "aaaaaaaaaaaa", 1001);
    let second = registration(root.path(), "bbbbbbbbbbbb", 1002);
    assert!(bind_session(root.path(), &ancestors_of(1001), "ses-a", Some("startup")).unwrap());
    assert!(bind_session(root.path(), &ancestors_of(1002), "ses-b", Some("startup")).unwrap());
    assert_eq!(
        binding_for(root.path(), "ses-a").unwrap().unwrap().env["YINZ_PANE_REF"],
        "aaaaaaaaaaaa"
    );
    assert_eq!(
        binding_for(root.path(), "ses-b").unwrap().unwrap().env["YINZ_PANE_REF"],
        "bbbbbbbbbbbb"
    );
    // A stranger's session binds nothing and routes nowhere: subagents, observers
    // and manual runs must not flip a pane's record.
    assert!(!bind_session(root.path(), &ancestors_of(9999), "ses-c", Some("startup")).unwrap());
    assert!(binding_for(root.path(), "ses-c").unwrap().is_none());
    assert!(binding_for(root.path(), "missing").unwrap().is_none());
    // A dead launcher cannot own a session, even after an unclean exit.
    std::fs::rename(first.registry.dir.path(), root.path().join("dead")).unwrap();
    drop(first);
    assert!(binding_for(root.path(), "ses-a").unwrap().is_none());
    assert!(!bind_session(root.path(), &ancestors_of(1001), "ses-d", Some("startup")).unwrap());
    drop(second);
    assert!(binding_for(root.path(), "ses-b").unwrap().is_none());
}

#[test]
fn the_first_session_start_wins_unless_the_session_changed() {
    let root = tempfile::tempdir().unwrap();
    let _live = registration(root.path(), "aaaaaaaaaaaa", 1001);
    let ancestors = ancestors_of(1001);
    assert!(bind_session(root.path(), &ancestors, "ses-a", Some("startup")).unwrap());
    // A second startup - a subagent announcing its own session - keeps the first.
    assert!(bind_session(root.path(), &ancestors, "ses-b", Some("startup")).unwrap());
    assert!(bind_session(root.path(), &ancestors, "ses-b", None).unwrap());
    assert_eq!(
        binding_for(root.path(), "ses-a").unwrap().unwrap().env["YINZ_PANE_REF"],
        "aaaaaaaaaaaa"
    );
    assert!(binding_for(root.path(), "ses-b").unwrap().is_none());
    // A resume, a fork or a clear says the session itself changed, and rebinds.
    for source in ["resume", "fork", "clear"] {
        assert!(bind_session(root.path(), &ancestors, "ses-next", Some(source)).unwrap());
        assert!(binding_for(root.path(), "ses-a").unwrap().is_none());
        let binding = binding_for(root.path(), "ses-next").unwrap().unwrap();
        assert_eq!(binding.env["YINZ_PANE_REF"], "aaaaaaaaaaaa");
        assert!(bind_session(root.path(), &ancestors, "ses-a", Some(source)).unwrap());
    }
    // A start from outside the session's own ancestry binds nothing, even naming
    // the bound session: only the session's own hooks can touch its binding.
    assert!(!bind_session(root.path(), &ancestors_of(4242), "ses-a", Some("startup")).unwrap());
    for bad in ["", &"s".repeat(257)] {
        bind_session(root.path(), &ancestors, bad, Some("startup")).unwrap_err();
    }
}

#[test]
fn reports_reach_the_bound_pane_and_nothing_else() {
    const CHILD_ENV: &str = "YINZ_TEST_MUSE_REPORT_CHILD";
    const TEST: &str = "tests::reports_reach_the_bound_pane_and_nothing_else";
    if std::env::var_os(CHILD_ENV).is_none() {
        let scratch = tempfile::tempdir().unwrap();
        let delivered = scratch.path().join("delivered");
        let fake = scratch.path().join("yinz");
        yinz_test_support::write_warm_script(
            &fake,
            &format!(
                "echo \"ARGS:$@\" >> {}\necho \"REF:$YINZ_PANE_REF ID:$YINZ_PANE_ID\" >> {}\ncat >> {}\necho >> {}",
                delivered.display(),
                delivered.display(),
                delivered.display(),
                delivered.display()
            ),
        );
        // This test process is the launcher, so the reporting child's own ancestry
        // binds through it without any registration changes after spawning.
        let pane = uuid::Uuid::new_v4().to_string();
        let _live = Registration::new(
            &directory(),
            HashMap::from([
                ("YINZ_PANE_REF".into(), pane.clone()),
                ("YINZ_PANE_ID".into(), "terminal_9".into()),
            ]),
            std::process::id(),
        )
        .unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        // A prompt `agent prompt` accepts, over the broadcast cap: the routed
        // report carries the pruned acknowledgement, not the whole prompt.
        let oversized = "review\n".repeat(100 * 1024);
        for event in [
            serde_json::json!({
                "hook_event_name": "SessionStart",
                "session_id": session,
                "cwd": "/work",
                "source": "startup",
            }),
            serde_json::json!({
                "hook_event_name": "Stop",
                "session_id": session,
                "cwd": "/work",
                "turn_id": "turn_1",
                "last_assistant_message": "done",
            }),
            serde_json::json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": session,
                "cwd": "/work",
                "prompt": oversized,
                "turn_id": "turn_1",
            }),
        ] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST])
                .env(CHILD_ENV, &fake)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(event.to_string().as_bytes())
                .unwrap();
            assert!(child.wait().unwrap().success());
        }
        // A stranger's event is dropped, silently and successfully: the fake yinz
        // sees exactly the three bound reports.
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST])
            .env(CHILD_ENV, &fake)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(
                serde_json::json!({
                    "hook_event_name": "Stop",
                    "session_id": uuid::Uuid::new_v4().to_string(),
                    "cwd": "/work",
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();
        assert!(child.wait().unwrap().success());
        let delivered = std::fs::read_to_string(&delivered).unwrap();
        assert_eq!(
            delivered
                .lines()
                .filter(|line| line.starts_with("ARGS:"))
                .count(),
            3
        );
        assert!(delivered.contains("action agent-report --hook-json --kind muse"));
        assert!(delivered.contains(&pane));
        assert!(delivered.contains(&session));
        assert!(delivered.contains("\"hook_event_name\":\"Stop\""));
        let submit = delivered
            .lines()
            .find(|line| line.contains("\"hook_event_name\":\"UserPromptSubmit\""))
            .unwrap();
        assert!(submit.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
        assert!(submit.contains("\"prompt_sha256\""));
        assert!(submit.contains("\"turn_id\":\"turn_1\""));
        return;
    }
    // The reporting child: stdin carries the hook, the fake yinz takes the report.
    report(Path::new(&std::env::var(CHILD_ENV).unwrap())).unwrap();
}

#[test]
fn the_launcher_registers_a_pane_and_runs_the_program() {
    const CHILD_ENV: &str = "YINZ_TEST_MUSE_LAUNCH_CHILD";
    const TEST: &str = "tests::the_launcher_registers_a_pane_and_runs_the_program";
    if std::env::var_os(CHILD_ENV).is_none() {
        for (pane, code) in [(false, 0), (false, 3), (true, 0), (true, 3)] {
            let mut launcher = Command::new(std::env::current_exe().unwrap());
            launcher
                .args(["--exact", TEST])
                .env(CHILD_ENV, code.to_string());
            if pane {
                launcher
                    .env("YINZ_PANE_REF", "aaaaaaaaaaaa")
                    .env("YINZ_PANE_ID", "terminal_9");
            } else {
                launcher
                    .env_remove("YINZ_PANE_REF")
                    .env_remove("YINZ_PANE_ID")
                    .env_remove("YINZ_REMOTE_PANE_TOKEN");
            }
            let output = launcher.output().unwrap();
            assert!(
                output.status.success(),
                "{pane}/{code}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
        return;
    }
    // Outside a pane the program runs directly; inside one it runs registered, and
    // the registration is gone when the program exits. The program snapshots the
    // registry mid-run, naming this child's pane ref, so siblings running in
    // parallel cannot perturb the assertion.
    let code: i32 = std::env::var(CHILD_ENV).unwrap().parse().unwrap();
    let pane = std::env::var("YINZ_PANE_REF").unwrap_or_default();
    let scratch = tempfile::tempdir().unwrap();
    let snapshot = scratch.path().join("snapshot");
    let program = scratch.path().join("agent");
    yinz_test_support::write_warm_script(
        &program,
        &format!(
            "grep -rl '{}' '{}' > '{}' 2>/dev/null; exit {code}",
            if pane.is_empty() {
                "no-such-pane"
            } else {
                &pane
            },
            directory().display(),
            snapshot.display()
        ),
    );
    assert_eq!(launch(&program, &[]).unwrap(), code);
    let seen = std::fs::read_to_string(&snapshot).unwrap();
    let error = launch(&scratch.path().join("missing-program"), &[]).unwrap_err();
    assert_eq!(error.to_string(), "run Muse");
    if pane.is_empty() {
        assert!(seen.is_empty(), "no registration outside a pane");
    } else {
        assert!(!seen.is_empty(), "the session runs registered");
        // Both an exited child and a failed spawn release their registration.
        let lingering = std::fs::read_dir(directory())
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| {
                        std::fs::read_to_string(entry.path().join("env.json"))
                            .is_ok_and(|env| env.contains(&pane))
                    })
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(lingering, 0);
    }
}

#[test]
fn muse_config_follows_xdg_config_home_and_falls_back_to_dot_config() {
    use std::ffi::OsString;
    assert_eq!(
        config_root(Some(OsString::from("/xdg")), || Ok(PathBuf::from(
            "/home/me"
        )),)
        .unwrap(),
        PathBuf::from("/xdg/muse")
    );
    assert_eq!(
        config_root(Some(OsString::new()), || Ok(PathBuf::from("/home/me"))).unwrap(),
        PathBuf::from("/home/me/.config/muse")
    );
    assert_eq!(
        config_root(None, || Ok(PathBuf::from("/home/me"))).unwrap(),
        PathBuf::from("/home/me/.config/muse")
    );
    config_root(None, || Err(anyhow!("no home"))).unwrap_err();
}

fn wired_config(root: &Path) -> (PathBuf, PathBuf) {
    let script = root.join("yinz").join(HOOK_SCRIPT_NAME);
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    yinz_test_support::write_warm_script(&script, "exit 0");
    let settings = root.join("settings.json");
    std::fs::write(
        &settings,
        yinz_types::agent::muse_hook_settings(&format!(
            "'{}'",
            script.display().to_string().replace('\'', r"'\''")
        ))
        .to_string(),
    )
    .unwrap();
    (script, settings)
}

#[test]
fn startup_verification_requires_every_event_through_an_executable_script() {
    let root = tempfile::tempdir().unwrap();
    let (script, settings) = wired_config(root.path());
    verify_hooks(&script, &settings).unwrap();
    // One event unwired leaves the pane unverified rather than misread.
    let mut partial: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    partial["hooks"]
        .as_object_mut()
        .unwrap()
        .remove(yinz_types::agent::MUSE_HOOK_EVENTS[0]);
    let partial_path = root.path().join("partial.json");
    std::fs::write(&partial_path, partial.to_string()).unwrap();
    verify_hooks(&script, &partial_path).unwrap_err();
    // A matcher filters occurrences, so a filtered handler reports nothing certain.
    let mut filtered: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    for groups in filtered["hooks"].as_object_mut().unwrap().values_mut() {
        for group in groups.as_array_mut().unwrap() {
            group["matcher"] = serde_json::json!("Notification");
        }
    }
    let filtered_path = root.path().join("filtered.json");
    std::fs::write(&filtered_path, filtered.to_string()).unwrap();
    verify_hooks(&script, &filtered_path).unwrap_err();
    // Someone else's handler is not a report to this binary.
    let foreign_path = root.path().join("foreign.json");
    std::fs::write(
        &foreign_path,
        yinz_types::agent::muse_hook_settings("someone-else").to_string(),
    )
    .unwrap();
    verify_hooks(&script, &foreign_path).unwrap_err();
    // The script must exist and run; the settings must exist and parse.
    verify_hooks(&root.path().join("missing.sh"), &settings).unwrap_err();
    let mut permissions = std::fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o644);
    std::fs::set_permissions(&script, permissions).unwrap();
    verify_hooks(&script, &settings).unwrap_err();
    let (script, _) = wired_config(&root.path().join("again"));
    verify_hooks(&script, &root.path().join("missing.json")).unwrap_err();
    let broken = root.path().join("broken.json");
    std::fs::write(&broken, "{not json").unwrap();
    verify_hooks(&script, &broken).unwrap_err();
}

#[test]
fn startup_verification_requires_the_exact_installed_command() {
    let root = tempfile::tempdir().unwrap();
    let (script, settings) = wired_config(&root.path().join("Muse's config with spaces"));
    verify_hooks(&script, &settings).unwrap();
    for command in [
        "'/stale/yinz/yinz-hook.sh'".to_owned(),
        format!("'{}'", root.path().join(HOOK_SCRIPT_NAME).display()),
        "echo yinz-hook.sh".to_owned(),
        format!("echo '{}'", script.display()),
        format!("'{}'", script.display()), // Unescaped apostrophe.
    ] {
        std::fs::write(
            &settings,
            yinz_types::agent::muse_hook_settings(&command).to_string(),
        )
        .unwrap();
        assert!(verify_hooks(&script, &settings).is_err(), "{command}");
    }
}

#[test]
fn startup_announcement_reports_verification_before_the_first_turn() {
    const CHILD_ENV: &str = "YINZ_TEST_MUSE_ANNOUNCE_CHILD";
    const TEST: &str = "tests::startup_announcement_reports_verification_before_the_first_turn";
    if std::env::var_os(CHILD_ENV).is_none() {
        let scratch = tempfile::tempdir().unwrap();
        for (name, wired) in [("wired", true), ("bare", false)] {
            let xdg = scratch.path().join(format!("xdg-{name}"));
            let config = xdg.join("muse");
            if wired {
                wired_config(&config);
            } else {
                std::fs::create_dir_all(&config).unwrap();
            }
            let delivered = scratch.path().join(format!("delivered-{name}"));
            let fake = scratch.path().join(format!("yinz-{name}"));
            yinz_test_support::write_warm_script(
                &fake,
                &format!(
                    "echo \"ARGS:$@\" >> {}\necho \"REF:$YINZ_PANE_REF ID:$YINZ_PANE_ID\" >> {}\ncat >> {}\necho >> {}",
                    delivered.display(),
                    delivered.display(),
                    delivered.display(),
                    delivered.display()
                ),
            );
            let pane = uuid::Uuid::new_v4().to_string();
            // The child resolves its config from the environment it inherits.
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST])
                .env(CHILD_ENV, format!("{}|{pane}", fake.display()))
                .env("XDG_CONFIG_HOME", &xdg)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            assert!(child.wait().unwrap().success());
            let delivered = std::fs::read_to_string(&delivered).unwrap();
            assert!(
                delivered.contains("action agent-report --hook-json --kind muse"),
                "{name}"
            );
            assert!(delivered.contains(&pane), "{name}");
            assert!(
                delivered.contains("\"hook_event_name\":\"SessionStart\""),
                "{name}"
            );
            assert!(delivered.contains("\"session_id\":\"\""), "{name}");
            assert!(
                delivered.contains(&format!("\"_yinz_muse_hooks_ready\":{wired}")),
                "{name}: {delivered}"
            );
        }
        return;
    }
    // The announcing child: its config root decides the marker it reports.
    let child_env = std::env::var(CHILD_ENV).unwrap();
    let (fake, pane) = child_env.split_once('|').unwrap();
    let live = Registration::new(
        &directory(),
        HashMap::from([
            ("YINZ_PANE_REF".into(), pane.to_owned()),
            ("YINZ_PANE_ID".into(), "terminal_9".into()),
        ]),
        std::process::id(),
    )
    .unwrap();
    live.announce(Path::new(fake));
}

#[test]
fn oversized_hooks_are_pruned_not_dropped() {
    // A prompt `agent prompt` accepts arrives whole inside `UserPromptSubmit`;
    // what travels on fits the broadcast cap and keeps the fingerprint the
    // acknowledgement is matched by.
    let prompt = "review\n".repeat(100 * 1024);
    assert!(prompt.len() > yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
    assert!(prompt.len() < yinz_types::agent::control::MAX_PROMPT_BYTES);
    let submit = serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "ses-a",
        "cwd": "/work",
        "prompt": prompt,
        "turn_id": "turn_1",
        "permission_mode": "default",
    })
    .to_string();
    let forwarded = prune_for_forward(submit).unwrap();
    assert!(forwarded.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
    let event: yinz_types::agent::HookEvent = serde_json::from_str(&forwarded).unwrap();
    assert_eq!(event.hook_event_name, "UserPromptSubmit");
    assert_eq!(event.turn_id.as_deref(), Some("turn_1"));
    assert_eq!(
        event.prompt_sha256.as_deref(),
        Some(yinz_types::agent::control::prompt_sha256(&prompt)).as_deref()
    );
    let broadcast = yinz_types::agent::AgentEvent::from_hook(
        "muse",
        yinz_types::internal_ids::TerminalId::new(7),
        &event,
        forwarded,
    );
    let mut observation =
        yinz_types::agent::control::PromptObservation::new("muse", "ses-a", &prompt);
    assert_eq!(
        observation.observe(
            &broadcast,
            serde_json::from_str::<yinz_types::agent::HookEvent>(&broadcast.raw)
                .ok()
                .as_ref(),
        ),
        Some(yinz_types::agent::control::AgentOutcome::Submitted)
    );

    // A large final message keeps the completion and still idles the pane.
    let stop = serde_json::json!({
        "hook_event_name": "Stop",
        "session_id": "ses-a",
        "cwd": "/work",
        "turn_id": "turn_1",
        "last_assistant_message": "done\n".repeat(100 * 1024),
    })
    .to_string();
    let forwarded = prune_for_forward(stop).unwrap();
    assert!(forwarded.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
    let event: yinz_types::agent::HookEvent = serde_json::from_str(&forwarded).unwrap();
    assert_eq!(event.hook_event_name, "Stop");
    assert_eq!(event.turn_id.as_deref(), Some("turn_1"));
    assert_eq!(
        yinz_types::agent::hook_state(&event),
        Some(yinz_types::agent::AgentState::Idle)
    );

    // What fits travels on byte-identical; what cannot parse still fails.
    let small = serde_json::json!({
        "hook_event_name": "Stop",
        "session_id": "ses-a",
        "turn_id": "turn_1",
    })
    .to_string();
    assert_eq!(prune_for_forward(small.clone()).unwrap(), small);
    let garbage = format!(
        "{{\"hook_event_name\": \"Stop\", \"padding\": \"{}",
        "x".repeat(yinz_types::agent::HOOK_BROADCAST_MAX_BYTES)
    );
    prune_for_forward(garbage).unwrap_err();
}

#[test]
fn forwarding_names_the_route_the_binding_carries() {
    let scratch = tempfile::tempdir().unwrap();
    let delivered = scratch.path().join("delivered");
    let fake = scratch.path().join("yinz");
    yinz_test_support::write_warm_script(
        &fake,
        &format!(
            "echo \"ARGS:$@\" >> {}\ncat >> {}",
            delivered.display(),
            delivered.display()
        ),
    );
    let raw = serde_json::json!({
        "hook_event_name": "Stop",
        "session_id": "ses-a",
        "cwd": "/work",
    })
    .to_string();
    forward(
        &fake,
        &HashMap::from([
            ("YINZ_PANE_REF".into(), "aaaaaaaaaaaa".into()),
            ("YINZ_PANE_ID".into(), "terminal_9".into()),
        ]),
        &raw,
    )
    .unwrap();
    forward(
        &fake,
        &HashMap::from([("YINZ_REMOTE_PANE_TOKEN".into(), "tok".into())]),
        &raw,
    )
    .unwrap();
    let delivered = std::fs::read_to_string(&delivered).unwrap();
    assert!(delivered.contains("ARGS:action agent-report --hook-json --kind muse"));
    assert!(delivered.contains("ARGS:--pty-broker report muse"));
    assert_eq!(delivered.matches("\"hook_event_name\":\"Stop\"").count(), 2);
}

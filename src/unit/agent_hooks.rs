use super::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "yinz-yinz-hooks-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

/// Installing wires every event, and installing twice wires each of them once.
/// Idempotence is by construction — the removal runs first — but the table has grown
/// and this is what says the growth reached the settings file.
#[test]
fn installing_wires_every_event_and_installing_twice_changes_nothing() {
    let command = "/home/me/.claude/yinz/yinz-hook.sh";
    let once = install_into(json!({}), command).expect("installed");
    let mut wired = installed_events(&once);
    wired.sort();
    let mut expected: Vec<String> = CLAUDE_EVENTS.iter().map(|e| (*e).to_owned()).collect();
    expected.sort();
    assert_eq!(wired, expected);

    // The three the summary and the toast now hang off, named rather than merely
    // counted: dropping one would leave a feed stuck on `busy` or a question silent.
    for event in ["Stop", "StopFailure", "PermissionRequest"] {
        assert!(CLAUDE_EVENTS.contains(&event), "{event}");
    }

    let twice = install_into(once.clone(), command).expect("installed again");
    assert_eq!(twice, once);
}

#[test]
fn generated_script_streams_a_large_payload_instead_of_putting_it_in_argv() {
    let temp = temp_dir("payload");
    let fake_yinz = temp.join("yinz");
    let hook = temp.join("yinz-hook.sh");
    let captured = temp.join("payload");
    let argc = temp.join("argc");
    std::fs::write(
        &fake_yinz,
        "#!/bin/sh\nprintf '%s' \"$#\" > \"$ARGC\"\ncat > \"$CAPTURE\"\n",
    )
    .unwrap();
    make_executable(&fake_yinz).unwrap();
    std::fs::write(&hook, script(HookAgent::Claude, &fake_yinz)).unwrap();
    make_executable(&hook).unwrap();

    let payload = vec![b'x'; 200 * 1024];
    let mut child = Command::new(&hook)
        .env_remove("YINZ_CLAUDE_INLINE_HOOKS")
        .env("YINZ_PANE_REF", "abcdef012345")
        .env_remove(yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV)
        .env("CAPTURE", &captured)
        .env("ARGC", &argc)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&payload).unwrap();
    assert!(child.wait().unwrap().success());

    assert_eq!(std::fs::read(captured).unwrap(), payload);
    assert_eq!(std::fs::read_to_string(argc).unwrap(), "3");
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn generated_codex_script_routes_without_daemon_pane_env_and_returns_neutral_json() {
    let temp = temp_dir("codex-payload");
    let fake_yinz = temp.join("yinz");
    let captured = temp.join("payload");
    let argc = temp.join("argc");
    std::fs::write(
        &fake_yinz,
        "#!/bin/sh\nprintf '%s' \"$#\" > \"$ARGC\"\ncat > \"$CAPTURE\"\n",
    )
    .unwrap();
    make_executable(&fake_yinz).unwrap();
    let hook = temp.join("yinz-hook.sh");
    std::fs::write(&hook, script(HookAgent::Codex, &fake_yinz)).unwrap();
    make_executable(&hook).unwrap();

    let output = Command::new(&hook)
        .env_remove("YINZ_PANE_REF")
        .env_remove(yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV)
        .env("CAPTURE", &captured)
        .env("ARGC", &argc)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"hook_event_name\":\"Stop\"}")?;
            child.wait_with_output()
        })
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}\n");
    assert_eq!(std::fs::read_to_string(argc).unwrap(), "2");
    assert_eq!(
        std::fs::read_to_string(captured).unwrap(),
        "{\"hook_event_name\":\"Stop\"}"
    );
    std::fs::remove_dir_all(temp).unwrap();
}

/// Run the generated script with a given environment and say what it invoked.
fn hook_argv(agent: HookAgent, name: &str, env: &[(&str, &str)]) -> (String, String) {
    let temp = temp_dir(name);
    let fake_yinz = temp.join("yinz");
    let captured = temp.join("payload");
    let argv = temp.join("argv");
    std::fs::write(
        &fake_yinz,
        "#!/bin/sh\nprintf '%s' \"$*\" > \"$ARGV\"\ncat > \"$CAPTURE\"\n",
    )
    .unwrap();
    make_executable(&fake_yinz).unwrap();
    let hook = temp.join("yinz-hook.sh");
    std::fs::write(&hook, script(agent, &fake_yinz)).unwrap();
    make_executable(&hook).unwrap();

    let mut command = Command::new(&hook);
    command
        .env_remove("YINZ_CLAUDE_INLINE_HOOKS")
        .env_remove("YINZ_PANE_REF")
        .env_remove(yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV)
        .env("CAPTURE", &captured)
        .env("ARGV", &argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command
        .spawn()
        .and_then(|mut child| {
            // A script with nothing to report exits without reading stdin, which is the
            // behaviour half of these cases are here to check - so losing the race to it
            // and getting EPIPE is a pass, not a failure. On a quiet machine the write
            // almost always wins, which is why this only ever showed up under load.
            match child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"hook_event_name\":\"Stop\"}")
            {
                Ok(()) => {},
                Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {},
                Err(err) => return Err(err),
            }
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());

    let argv = std::fs::read_to_string(&argv).unwrap_or_default();
    let payload = std::fs::read_to_string(&captured).unwrap_or_default();
    std::fs::remove_dir_all(temp).unwrap();
    (argv, payload)
}

#[test]
fn a_remote_pane_reports_through_its_broker() {
    let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
    let (argv, payload) = hook_argv(HookAgent::Claude, "remote-claude", &[(token, "tok")]);
    assert_eq!(argv, "--pty-broker report claude");
    assert_eq!(payload, "{\"hook_event_name\":\"Stop\"}");

    let (argv, _) = hook_argv(HookAgent::Codex, "remote-codex", &[(token, "tok")]);
    assert_eq!(argv, "agent codex-report");

    let (argv, payload) = hook_argv(HookAgent::Muse, "remote-muse", &[(token, "tok")]);
    assert_eq!(argv, "agent muse-report");
    assert_eq!(payload, "{\"hook_event_name\":\"Stop\"}");

    let (argv, payload) = hook_argv(HookAgent::Agy, "remote-agy", &[(token, "tok")]);
    assert_eq!(argv, "--pty-broker report agy");
    assert_eq!(payload, "{\"hook_event_name\":\"Stop\"}");
}

/// The order the two probes are made in is the behaviour: a remote pane carries a
/// `YINZ_PANE_ID` too, and it is the *asking* session's, on a machine with no such
/// session. Reading it would send every event to whatever session runs on this box.
#[test]
fn a_remote_pane_ignores_the_pane_id_it_also_carries() {
    let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
    let (argv, _) = hook_argv(
        HookAgent::Claude,
        "remote-both",
        &[(token, "tok"), ("YINZ_PANE_ID", "7")],
    );
    assert_eq!(argv, "--pty-broker report claude");
}

/// An empty token is no token: `env -u` is not the only way a variable goes away.
#[test]
fn neither_a_pane_id_nor_a_token_reports_nothing() {
    let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
    let (argv, _) = hook_argv(HookAgent::Claude, "no-pane", &[(token, "")]);
    assert!(argv.is_empty(), "{argv:?}");
}

#[test]
fn writing_new_settings_only_reports_a_backup_when_it_made_one() {
    let temp = temp_dir("backup");
    let settings = temp.join(".claude/settings.json");

    assert!(!write_settings(&settings, &json!({ "model": "opus" })).unwrap());
    assert!(!settings.with_extension("json.bak").exists());

    assert!(write_settings(&settings, &json!({ "model": "sonnet" })).unwrap());
    assert!(settings.with_extension("json.bak").exists());
    std::fs::remove_dir_all(temp).unwrap();
}

fn fake_yinz(temp: &Path) -> PathBuf {
    let fake = temp.join("yinz");
    std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&fake).unwrap();
    fake
}

fn project_files(temp: &Path) -> Files {
    Files {
        agent: HookAgent::Claude,
        script: temp.join("home/.claude/yinz").join(MARKER),
        user_settings: temp.join("home/.claude/settings.json"),
        project: Some(temp.join("project")),
    }
}

fn codex_files(temp: &Path) -> Files {
    Files {
        agent: HookAgent::Codex,
        script: temp.join("home/.codex/yinz").join(MARKER),
        user_settings: temp.join("home/.codex/hooks.json"),
        project: None,
    }
}

fn opencode_files(temp: &Path) -> Files {
    let plugin = temp
        .join("home/.config/opencode/plugin")
        .join(OPENCODE_PLUGIN_FILE);
    Files {
        agent: HookAgent::Opencode,
        script: plugin.clone(),
        user_settings: plugin,
        project: None,
    }
}

fn pi_files(temp: &Path) -> Files {
    let extension = temp
        .join("home/.pi/agent/extensions")
        .join(yinz_runtime::agent::pi::PI_EXTENSION_FILE);
    Files {
        agent: HookAgent::Pi,
        script: extension.clone(),
        user_settings: extension,
        project: None,
    }
}

fn muse_files(temp: &Path) -> Files {
    Files {
        agent: HookAgent::Muse,
        script: temp.join("home/.config/muse/yinz").join(MARKER),
        user_settings: temp.join("home/.config/muse/settings.json"),
        project: None,
    }
}

fn agy_files(temp: &Path) -> Files {
    Files {
        agent: HookAgent::Agy,
        script: temp.join("home/.gemini/yinz").join(MARKER),
        user_settings: temp.join("home/.gemini/config/hooks.json"),
        project: None,
    }
}

fn agy_project_files(temp: &Path) -> Files {
    Files {
        agent: HookAgent::Agy,
        script: temp.join("home/.gemini/yinz").join(MARKER),
        user_settings: temp.join("home/.gemini/config/hooks.json"),
        project: Some(temp.join("project")),
    }
}

fn events_in(path: &Path) -> Vec<String> {
    installed_events(&read_settings(path).unwrap())
}

#[test]
fn a_project_install_lands_in_settings_local_json() {
    let temp = temp_dir("local");
    let files = project_files(&temp);
    install(&files, &fake_yinz(&temp), false).unwrap();

    let local = temp.join("project/.claude/settings.local.json");
    assert_eq!(events_in(&local).len(), CLAUDE_EVENTS.len());
    assert!(
        !temp.join("project/.claude/settings.json").exists(),
        "the committed file is not ours to write"
    );
    assert!(!files.user_settings.exists());
    assert!(files.script.exists());
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn a_codex_install_lands_in_the_user_hooks_file() {
    let temp = temp_dir("codex user's home");
    let files = codex_files(&temp);
    install(&files, &fake_yinz(&temp), false).unwrap();

    assert_eq!(events_in(&files.user_settings).len(), CODEX_EVENTS.len());
    assert!(files.script.exists());
    assert!(!temp.join("home/.claude").exists());

    let settings = read_settings(&files.user_settings).unwrap();
    let command = settings["hooks"][CODEX_EVENTS[0]][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("YINZ_PANE_REF", "abcdef012345")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}\n");
    std::fs::remove_dir_all(temp).unwrap();
}

/// A Muse install wires the user settings file and guarantees the `schema_version`
/// Muse refuses to run without: created at 1 where missing, kept where present,
/// and left behind by uninstall, which only takes the wiring out.
#[test]
fn a_muse_install_lands_in_the_user_settings_file() {
    let temp = temp_dir("muse user's home");
    let files = muse_files(&temp);
    install(&files, &fake_yinz(&temp), false).unwrap();

    assert_eq!(events_in(&files.user_settings).len(), MUSE_EVENTS.len());
    assert!(files.script.exists());
    assert!(!temp.join("home/.claude").exists());
    let settings = read_settings(&files.user_settings).unwrap();
    assert_eq!(settings["schema_version"], json!(1));
    // The wired command reports with neither pane variable set, and stays silent:
    // Muse reads stdout as an optional decision object.
    let command = settings["hooks"][MUSE_EVENTS[0]][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env_remove("YINZ_PANE_REF")
        .env_remove(yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());

    // Installing twice wires each event once, and keeps an existing version and
    // every unrelated setting.
    std::fs::write(
        &files.user_settings,
        serde_json::to_string_pretty(&json!({
            "schema_version": 2,
            "model": "muse-spark",
        }))
        .unwrap(),
    )
    .unwrap();
    install(&files, &fake_yinz(&temp), false).unwrap();
    let settings = read_settings(&files.user_settings).unwrap();
    assert_eq!(settings["schema_version"], json!(2));
    assert_eq!(settings["model"], json!("muse-spark"));
    assert_eq!(events_in(&files.user_settings).len(), MUSE_EVENTS.len());

    uninstall(&files).unwrap();
    let settings = read_settings(&files.user_settings).unwrap();
    assert!(installed_events(&settings).is_empty());
    assert_eq!(settings["schema_version"], json!(2));
    assert!(!files.script.exists());
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn an_agy_install_lands_in_the_user_hooks_file() {
    let temp = temp_dir("agy user home");
    let files = agy_files(&temp);
    install(&files, &fake_yinz(&temp), false).unwrap();

    assert_eq!(
        files
            .installed_events(&read_settings(&files.user_settings).unwrap())
            .len(),
        AGY_EVENTS.len()
    );
    assert!(files.script.exists());

    let settings = read_settings(&files.user_settings).unwrap();
    assert!(settings.get("hooks").is_some());
    let pre_tool = &settings["hooks"]["PreToolUse"][0];
    assert_eq!(pre_tool["matcher"], ".*");
    assert!(pre_tool["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("PreToolUse"));

    let pre_inv = &settings["hooks"]["PreInvocation"][0];
    assert!(pre_inv["command"]
        .as_str()
        .unwrap()
        .contains("PreInvocation"));

    let output = Command::new("sh")
        .arg(&files.script)
        .arg("PreToolUse")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{\"decision\":\"ask\"}\n");

    let output_with_pane = Command::new("sh")
        .arg(&files.script)
        .arg("PreToolUse")
        .env("YINZ_PANE_REF", "abcdef012345")
        .output()
        .unwrap();
    assert!(output_with_pane.status.success());
    assert_eq!(output_with_pane.stdout, b"{\"decision\":\"ask\"}\n");

    let output = Command::new("sh")
        .arg(&files.script)
        .arg("Stop")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{\"decision\":\"stop\"}\n");

    let output = Command::new("sh")
        .arg(&files.script)
        .arg("PreInvocation")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}\n");

    uninstall(&files).unwrap();
    assert!(files
        .installed_events(&read_settings(&files.user_settings).unwrap())
        .is_empty());
    assert!(!files.script.exists());
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn an_agy_project_install_lands_in_agents_hooks_json() {
    let temp = temp_dir("agy project");
    let files = agy_project_files(&temp);
    install(&files, &fake_yinz(&temp), false).unwrap();

    let local = temp.join("project/.agents/hooks.json");
    assert_eq!(
        files
            .installed_events(&read_settings(&local).unwrap())
            .len(),
        AGY_EVENTS.len()
    );
    assert!(files.script.exists());
    assert!(!files.user_settings.exists());

    uninstall(&files).unwrap();
    assert!(files
        .installed_events(&read_settings(&local).unwrap())
        .is_empty());
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn installing_agy_hooks_preserves_existing_handlers_and_uninstall_retains_them() {
    let command = "/home/me/.gemini/yinz/yinz-hook.sh";
    let original = json!({
        "other_block": {
            "SomeEvent": [{ "command": "echo other" }]
        },
        "hooks": {
            "Stop": [
                { "command": "/usr/local/bin/custom-stop.sh", "timeout": 10 }
            ],
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [
                        { "command": "/usr/local/bin/tool-guard.sh", "timeout": 5 }
                    ]
                }
            ],
            "CustomEvent": [
                { "command": "/usr/local/bin/custom.sh" }
            ]
        }
    });

    let installed = install_agy_into(original.clone(), command).expect("installed agy hooks");

    // Other block and unrelated custom event are completely untouched
    assert_eq!(installed["other_block"], original["other_block"]);
    assert_eq!(
        installed["hooks"]["CustomEvent"],
        original["hooks"]["CustomEvent"]
    );

    // Custom Stop command is retained at index 0, our handler appended at index 1
    let stop = installed["hooks"]["Stop"].as_array().expect("array");
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0]["command"], "/usr/local/bin/custom-stop.sh");
    assert_eq!(stop[1]["command"], format!("{command} Stop"));

    // Custom PreToolUse matcher is retained at index 0, our .* matcher appended at index 1
    let pre_tool = installed["hooks"]["PreToolUse"].as_array().expect("array");
    assert_eq!(pre_tool.len(), 2);
    assert_eq!(pre_tool[0]["matcher"], "Bash");
    assert_eq!(pre_tool[1]["matcher"], ".*");
    assert_eq!(
        pre_tool[1]["hooks"][0]["command"],
        format!("{command} PreToolUse")
    );

    // Every other AGY event has our handler as a 1-element array
    let wired = installed_agy_events(&installed);
    assert_eq!(wired.len(), AGY_EVENTS.len());
    for event in AGY_EVENTS {
        assert!(wired.contains(&event.to_string()), "{event} not wired");
    }

    // Reinstalling is idempotent: neither custom hooks nor our hooks are duplicated
    let twice = install_agy_into(installed.clone(), command).expect("installed twice");
    assert_eq!(twice, installed);

    // Uninstalling removes only our hooks, leaving all custom handlers and blocks intact
    let uninstalled = uninstall_agy_from(installed);
    assert_eq!(uninstalled, original);
    assert!(installed_agy_events(&uninstalled).is_empty());
}

#[test]
fn installing_agy_hooks_rejects_settings_shapes_it_cannot_merge() {
    assert!(install_agy_into(json!({ "hooks": [] }), "yinz-hook.sh").is_err());
    assert!(install_agy_into(
        json!({ "hooks": { "Stop": { "not": "an array" } } }),
        "yinz-hook.sh"
    )
    .is_err());
}

/// opencode's wiring is one plugin file and no settings merge, so installing twice is
/// one file either way and uninstalling takes it with it.
#[test]
fn an_opencode_install_writes_a_versioned_plugin_and_uninstall_removes_it() {
    let temp = temp_dir("opencode-install");
    let files = opencode_files(&temp);
    install(&files, &fake_yinz(&temp), false).unwrap();

    let plugin = read_generated(&files.script).unwrap().unwrap();
    assert!(
        plugin.contains(&format!("yinz-opencode-plugin v{OPENCODE_PLUGIN_VERSION}")),
        "{plugin}"
    );
    assert!(plugin.contains("\"agent-report\""), "{plugin}");
    assert!(plugin.contains("\"--pty-broker\""), "{plugin}");
    assert!(
        !plugin.contains("__YINZ_BIN__")
            && !plugin.contains("__TOKEN_ENV__")
            && !plugin.contains("__VERSION__"),
        "every substitution is filled in"
    );
    assert!(!temp.join("home/.config/opencode/settings.json").exists());

    install(&files, &fake_yinz(&temp), false).unwrap();
    assert_eq!(read_generated(&files.script).unwrap().unwrap(), plugin);

    uninstall(&files).unwrap();
    assert!(read_generated(&files.script).unwrap().is_none());
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn opencode_plugin_reports_in_order_and_tracks_turn_text() {
    let temp = tempfile::tempdir().unwrap();
    let plugin = temp.path().join("yinz.mjs");
    let harness = temp.path().join("regressions.mjs");
    std::fs::write(&plugin, opencode_plugin(Path::new("/test/yinz"))).unwrap();
    std::fs::write(&harness, include_str!("opencode_plugin.mjs")).unwrap();
    let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
    for remote in [false, true] {
        let mut command = Command::new("node");
        command
            .arg(&harness)
            .arg(&plugin)
            .env("YINZ_PANE_REF", "abcdef012345");
        if remote {
            command.env(token, "test-token");
        } else {
            command.env_remove(token);
        }
        let output = command
            .output()
            .expect("Node is required for the OpenCode plugin tests");
        assert!(
            output.status.success(),
            "OpenCode plugin regression failed (remote={remote}):\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

/// A plugin file we did not write is somebody else's: neither install nor uninstall may
/// touch it, and both say so.
#[test]
fn opencode_leaves_a_plugin_file_it_did_not_write_alone() {
    let temp = temp_dir("opencode-foreign");
    let files = opencode_files(&temp);
    std::fs::create_dir_all(files.script.parent().unwrap()).unwrap();
    std::fs::write(&files.script, "// somebody else's plugin\n").unwrap();

    assert!(install(&files, &fake_yinz(&temp), false).is_err());
    assert!(uninstall(&files).is_err());
    assert_eq!(
        read_generated(&files.script).unwrap().unwrap(),
        "// somebody else's plugin\n"
    );
    std::fs::remove_dir_all(temp).unwrap();
}

/// The binary path is a JavaScript string, not a shell word: a folder name with a space
/// or a quote in it must not write a plugin opencode cannot parse.
#[test]
fn the_opencode_plugin_bakes_a_json_quoted_binary_path() {
    let plugin = opencode_plugin(Path::new("/opt/my yinz/yinz"));
    assert!(plugin.contains("\"/opt/my yinz/yinz\""), "{plugin}");
}

/// pi's wiring is one extension file in pi's own extension directory, and it is the
/// very file a spawned pane is handed inline - so installing twice writes the same
/// bytes, and uninstalling takes the file with it.
#[test]
fn pi_extension_reports_lifecycle_and_recovers_across_reload() {
    let temp = tempfile::tempdir().unwrap();
    let extension = temp.path().join("reporter.mts");
    let harness = temp.path().join("harness.mjs");
    std::fs::write(&extension, pi_extension(Path::new("/test/yinz"))).unwrap();
    std::fs::write(&harness, include_str!("pi_extension.mjs")).unwrap();
    for remote in [false, true] {
        let mut command = Command::new("node");
        command
            .arg(&harness)
            .arg(&extension)
            .env("YINZ_PANE_ID", "1");
        if remote {
            command.env("YINZ_REMOTE_PANE_TOKEN", "test-token");
        } else {
            command.env_remove("YINZ_REMOTE_PANE_TOKEN");
        }
        let output = command
            .output()
            .expect("Node is required for Pi extension tests");
        assert!(
            output.status.success(),
            "Pi reporter (remote={remote}): {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn claude_install_directory_honors_override_and_empty_fallback() {
    assert_eq!(
        agent_root(
            HookAgent::Claude,
            Some("/custom/claude".into()),
            None,
            None,
            None,
            || panic!("override does not need HOME")
        )
        .unwrap(),
        PathBuf::from("/custom/claude")
    );
    for value in [None, Some(OsString::new())] {
        assert_eq!(
            agent_root(HookAgent::Claude, value, None, None, None, || Ok(
                PathBuf::from("/home/test")
            ))
            .unwrap(),
            PathBuf::from("/home/test/.claude")
        );
    }
}

#[test]
fn claude_hook_files_resolve_inside_the_override() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", temp.path()) };
    let files = Files::resolve(HookAgent::Claude, false, None).unwrap();
    assert_eq!(files.script, temp.path().join("yinz").join(MARKER));
    assert_eq!(files.user_settings, temp.path().join("settings.json"));
}

#[test]
fn pi_install_directory_override_does_not_need_home() {
    for path in ["/custom/pi", "relative/pi", "~user/pi", "./~/pi"] {
        assert_eq!(
            agent_root(HookAgent::Pi, None, None, Some(path.into()), None, || {
                panic!("override does not need HOME")
            })
            .unwrap(),
            PathBuf::from(path)
        );
    }
    assert_eq!(
        agent_root(HookAgent::Pi, None, None, Some("".into()), None, || Ok(
            PathBuf::from("/home/test")
        ))
        .unwrap(),
        PathBuf::from("/home/test/.pi/agent")
    );
}

#[test]
fn pi_install_directory_override_expands_tilde() {
    for (path, expected) in [
        ("~", "/home/test"),
        ("~/.config/pi", "/home/test/.config/pi"),
    ] {
        assert_eq!(
            agent_root(HookAgent::Pi, None, None, Some(path.into()), None, || Ok(
                PathBuf::from("/home/test")
            ))
            .unwrap(),
            PathBuf::from(expected)
        );
        assert!(
            agent_root(HookAgent::Pi, None, None, Some(path.into()), None, || Err(
                anyhow!("HOME is not set")
            ))
            .is_err()
        );
    }
}

#[test]
fn a_pi_install_writes_the_shared_extension_and_uninstall_removes_it() {
    let temp = temp_dir("pi-install");
    let files = pi_files(&temp);
    install(&files, &fake_yinz(&temp), false).unwrap();

    let extension = read_generated(&files.script).unwrap().unwrap();
    assert!(
        extension.contains(&format!(
            "{} v{}",
            yinz_runtime::agent::pi::PI_EXTENSION_MARKER,
            yinz_runtime::agent::pi::PI_EXTENSION_VERSION
        )),
        "{extension}"
    );
    // The routes the spawned copy keeps too: the pane report locally, the broker
    // remotely, and nothing at all outside a yinz pane.
    assert!(extension.contains("\"--kind\", \"pi\""), "{extension}");
    assert!(extension.contains("--pty-broker"), "{extension}");
    assert!(extension.contains("YINZ_PANE_ID"), "{extension}");
    assert!(
        !extension.contains("__YINZ_BIN__")
            && !extension.contains("__TOKEN_ENV__")
            && !extension.contains("__VERSION__"),
        "every substitution is filled in"
    );
    // pi needs no settings: nothing but the extension was written.
    assert!(!temp.join("home/.pi/agent/settings.json").exists());

    install(&files, &fake_yinz(&temp), false).unwrap();
    assert_eq!(read_generated(&files.script).unwrap().unwrap(), extension);

    uninstall(&files).unwrap();
    assert!(read_generated(&files.script).unwrap().is_none());
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn pi_install_dry_run_preserves_files_and_real_install_keeps_a_backup() {
    let temp = tempfile::tempdir().unwrap();
    let files = pi_files(temp.path());
    install(&files, Path::new("/bin/yinz"), true).unwrap();
    assert!(!files.script.exists());
    install(&files, Path::new("/bin/yinz"), false).unwrap();
    let original = std::fs::read_to_string(&files.script).unwrap();
    install(&files, Path::new("/other/yinz"), true).unwrap();
    assert_eq!(std::fs::read_to_string(&files.script).unwrap(), original);
    install(&files, Path::new("/other/yinz"), false).unwrap();
    assert_eq!(
        std::fs::read_to_string(files.script.with_extension("ts.bak")).unwrap(),
        original
    );
}

#[test]
fn pi_leaves_an_extension_it_did_not_write_alone() {
    let temp = temp_dir("pi-foreign");
    let files = pi_files(&temp);
    std::fs::create_dir_all(files.script.parent().unwrap()).unwrap();
    std::fs::write(&files.script, "// somebody else's extension\n").unwrap();

    assert!(install(&files, &fake_yinz(&temp), false).is_err());
    assert!(uninstall(&files).is_err());
    assert_eq!(
        read_generated(&files.script).unwrap().unwrap(),
        "// somebody else's extension\n"
    );
    std::fs::remove_dir_all(temp).unwrap();
}

/// The default install covers every agent, and pi has no project install: `.pi/`
/// is repo content pi itself will not load without the user's trust.
#[test]
fn pi_is_part_of_the_default_install_and_never_a_project_one() {
    let every = selected_agents(None, false).unwrap();
    assert!(every.contains(&HookAgent::Pi));
    assert_eq!(
        selected_agents(None, true).unwrap(),
        vec![HookAgent::Claude]
    );
    assert!(selected_agents(Some(HookAgent::Pi), true).is_err());
    assert_eq!(
        agent_root(HookAgent::Pi, None, None, None, None, || Ok(PathBuf::from(
            "/home/me"
        )))
        .unwrap(),
        PathBuf::from("/home/me/.pi/agent")
    );
    assert!(Files::resolve(HookAgent::Pi, true, None).is_err());
}

#[test]
fn a_custom_codex_home_does_not_require_home() {
    assert_eq!(
        agent_root(
            HookAgent::Codex,
            None,
            Some(OsString::from("/custom/codex")),
            None,
            None,
            || Err(anyhow!("HOME is unavailable"))
        )
        .unwrap(),
        PathBuf::from("/custom/codex")
    );
}

/// `export CODEX_HOME="$UNSET"` used to resolve to `""`, and an install into `""` writes
/// `hooks.json` and `yinz/` into whichever folder the person was standing in. An empty
/// variable is an unset one.
#[test]
fn an_empty_codex_home_falls_back_to_home_rather_than_to_the_current_folder() {
    assert_eq!(
        agent_root(
            HookAgent::Codex,
            None,
            Some(OsString::new()),
            None,
            None,
            || Ok(PathBuf::from("/home/me"))
        )
        .unwrap(),
        PathBuf::from("/home/me/.codex")
    );
    assert!(agent_root(
        HookAgent::Codex,
        None,
        Some(OsString::new()),
        None,
        None,
        || Err(anyhow!("HOME is unavailable"))
    )
    .is_err());
}

/// opencode's config root is the one that follows `XDG_CONFIG_HOME`, and its empty
/// value is an unset one the same way `CODEX_HOME`'s is.
#[test]
fn opencode_honours_xdg_config_home_and_falls_back_to_dot_config() {
    assert_eq!(
        agent_root(
            HookAgent::Opencode,
            None,
            None,
            None,
            Some(OsString::from("/xdg")),
            || Ok(PathBuf::from("/home/me"))
        )
        .unwrap(),
        PathBuf::from("/xdg/opencode")
    );
    assert_eq!(
        agent_root(
            HookAgent::Opencode,
            None,
            None,
            None,
            Some(OsString::new()),
            || Ok(PathBuf::from("/home/me"))
        )
        .unwrap(),
        PathBuf::from("/home/me/.config/opencode")
    );
    assert_eq!(
        agent_root(HookAgent::Opencode, None, None, None, None, || Ok(
            PathBuf::from("/home/me")
        ))
        .unwrap(),
        PathBuf::from("/home/me/.config/opencode")
    );
}

/// Muse's config root follows `XDG_CONFIG_HOME` the same way, under `muse`.
#[test]
fn muse_honours_xdg_config_home_and_falls_back_to_dot_config() {
    assert_eq!(
        agent_root(
            HookAgent::Muse,
            None,
            None,
            None,
            Some(OsString::from("/xdg")),
            || Ok(PathBuf::from("/home/me"))
        )
        .unwrap(),
        PathBuf::from("/xdg/muse")
    );
    assert_eq!(
        agent_root(
            HookAgent::Muse,
            None,
            None,
            None,
            Some(OsString::new()),
            || Ok(PathBuf::from("/home/me"))
        )
        .unwrap(),
        PathBuf::from("/home/me/.config/muse")
    );
    assert_eq!(
        agent_root(HookAgent::Muse, None, None, None, None, || Ok(
            PathBuf::from("/home/me")
        ))
        .unwrap(),
        PathBuf::from("/home/me/.config/muse")
    );
}

/// The default form of every one of these commands does both agents, so a broken
/// `~/.claude/settings.json` - Claude goes first - must not leave Codex unwired and
/// unmentioned. It still ends in an error, because something really did fail.
#[test]
fn one_agents_failure_does_not_stop_the_other() {
    let mut ran = Vec::new();
    let result = for_each_agent(vec![HookAgent::Claude, HookAgent::Codex], |agent| {
        ran.push(agent);
        match agent {
            HookAgent::Claude => Err(anyhow!("settings.json is not valid JSON")),
            _ => Ok(()),
        }
    });
    assert_eq!(ran, vec![HookAgent::Claude, HookAgent::Codex]);
    assert_eq!(
        result.unwrap_err().to_string(),
        "Claude Code failed - see above"
    );

    assert!(for_each_agent(vec![HookAgent::Claude, HookAgent::Codex], |_| Ok(())).is_ok());
}

#[test]
fn user_commands_default_to_every_agent_but_project_commands_to_claude() {
    assert_eq!(
        selected_agents(None, false).unwrap(),
        vec![
            HookAgent::Claude,
            HookAgent::Codex,
            HookAgent::Opencode,
            HookAgent::Muse,
            HookAgent::Agy,
            HookAgent::Pi,
        ]
    );
    assert_eq!(
        selected_agents(None, true).unwrap(),
        vec![HookAgent::Claude]
    );
    assert_eq!(
        selected_agents(Some(HookAgent::Agy), true).unwrap(),
        vec![HookAgent::Agy]
    );
    assert!(selected_agents(Some(HookAgent::Codex), true).is_err());
    assert!(selected_agents(Some(HookAgent::Opencode), true).is_err());
    assert!(selected_agents(Some(HookAgent::Muse), true).is_err());
    assert!(selected_agents(Some(HookAgent::All), true).is_err());
}

#[test]
fn a_project_install_moves_entries_out_of_the_committed_file() {
    let temp = temp_dir("legacy");
    let files = project_files(&temp);
    let legacy = temp.join("project/.claude/settings.json");
    let command = files.script.display().to_string();
    let old = install_into(json!({ "model": "opus" }), &command).unwrap();
    write_settings(&legacy, &old).unwrap();

    install(&files, &fake_yinz(&temp), false).unwrap();

    assert!(
        events_in(&legacy).is_empty(),
        "the old install is gone from the committed file"
    );
    assert_eq!(read_settings(&legacy).unwrap()["model"], "opus");
    assert_eq!(
        events_in(&temp.join("project/.claude/settings.local.json")).len(),
        CLAUDE_EVENTS.len()
    );
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn a_project_uninstall_keeps_the_script_a_user_install_uses() {
    let temp = temp_dir("shared-script");
    let yinz = fake_yinz(&temp);
    let project = project_files(&temp);
    let user = Files {
        project: None,
        ..project_files(&temp)
    };
    install(&user, &yinz, false).unwrap();
    install(&project, &yinz, false).unwrap();

    uninstall(&project).unwrap();
    assert!(events_in(&project.settings()).is_empty());
    assert_eq!(events_in(&user.user_settings).len(), CLAUDE_EVENTS.len());
    assert!(
        project.script.exists(),
        "the user install still points at it"
    );

    uninstall(&user).unwrap();
    assert!(!user.script.exists());
    std::fs::remove_dir_all(temp).unwrap();
}

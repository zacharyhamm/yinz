use super::*;
use serde_json::json;

#[test]
fn writable_root_overrides_bypass_the_bridge_in_every_config_spelling() {
    for (value, expected) in [
        ("sandbox_workspace_write.writable_roots=['/work']", false),
        (
            " sandbox_workspace_write.writable_roots \t = [ '/work with spaces' ]",
            false,
        ),
        ("sandbox_workspace_write.writable_roots=[]", false),
        ("model='sandbox_workspace_write.writable_roots'", true),
        ("sandbox_workspace_write.network_access=true", true),
        ("other.sandbox_workspace_write.writable_roots=[]", true),
        ("sandbox_workspace_write.writable_roots_extra=[]", true),
    ] {
        assert!(bridge_options(&["--model".into(), value.into()]).is_some());
        for args in [
            vec!["-c".into(), value.into()],
            vec!["--config".into(), value.into()],
            vec![format!("-c{value}")],
            vec![format!("-c={value}")],
            vec![format!("--config={value}")],
        ] {
            assert_eq!(bridge_options(&args).is_some(), expected, "{args:?}");
            let mut literal = vec!["--".into()];
            literal.extend(args);
            assert!(bridge_options(&literal).is_some(), "{literal:?}");
        }
    }
}

#[test]
fn bridge_eligibility_consumes_options_before_identifying_commands() {
    let cases: &[(&[&str], bool)] = &[
        (&[], true),
        (&["fix the parser"], true),
        (&["--config", "model=review", "fix the parser"], true),
        (&["-c", "model=review", "exec", "task"], false),
        (&["--config=model=review", "e", "task"], false),
        (&["--profile", "work", "login"], false),
        (&["--profile=work", "app-server", "daemon", "start"], false),
        (&["-pwork", "a", "task-id"], false),
        (&["-p=work", "agents"], false),
        (&["--enable", "feature", "cloud"], false),
        (&["--disable=feature", "features", "list"], false),
        (&["--no-alt-screen", "completion", "bash"], false),
        (&["--model", "exec"], true),
        (&["--model=exec"], true),
        (&["-mexec"], true),
        (&["-m=exec"], true),
        (&["-p", "login", "prompt"], true),
        (&["-C", "review", "prompt"], true),
        (&["-cmodel=exec", "-sworkspace-write", "-anever"], true),
        (&["--local-provider", "ollama", "--oss"], true),
        (&["--", "exec"], true),
        (&["--", "--remote=literal prompt"], true),
        (&["--", "--add-dir"], true),
        (&["--cd=exec", "--", "--help"], true),
        (&["prompt", "--no-alt-screen"], true),
        (&["resume"], true),
        (&["fork"], true),
        (&["--model", "exec", "resume", "--last"], true),
        (&["--profile=work", "fork", "--all"], true),
        (&["resume", "login", "prompt"], true),
        (&["fork", "exec", "--model", "review", "prompt"], true),
        (&["resume", "--include-non-interactive", "--last"], true),
        (&["resume", "--", "--no-daemon", "prompt"], true),
        (&["--add-dir", "/work"], false),
        (&["--add-dir=/work"], false),
        (&["resume", "--last", "--add-dir=/work"], false),
        (&["--remote", "unix:///tmp/codex.sock"], false),
        (&["--remote=unix:///tmp/codex.sock"], false),
        (&["--remote-auth-token-env=TOKEN"], false),
        (&["fork", "--no-daemon"], false),
        (&["--help"], false),
        (&["-h"], false),
        (&["--version"], false),
        (&["-V"], false),
        (&["-ch"], true),
        (&["-hcmodel=exec"], false),
        (&["--model"], false),
        (&["--model="], false),
        (&["--model", "--", "exec"], false),
        (&["--new-option", "exec"], false),
        (&["prompt", "--new-option"], false),
        (&["-x", "exec"], false),
        (&["--no-alt-screen=false"], false),
        (&["--image", "one.png", "exec"], false),
        (&["-ione.png", "exec"], false),
    ];
    for (args, expected) in cases {
        let args: Vec<_> = args.iter().map(|arg| (*arg).to_owned()).collect();
        assert_eq!(bridge_options(&args).is_some(), *expected, "{args:?}");
    }
    for utility in [
        "agents",
        "exec",
        "e",
        "review",
        "login",
        "logout",
        "mcp",
        "plugin",
        "app-server",
        "remote-control",
        "completion",
        "update",
        "doctor",
        "sandbox",
        "debug",
        "apply",
        "a",
        "queue",
        "archive",
        "delete",
        "migrate-rollouts",
        "unarchive",
        "features",
        "exec-server",
        "execpolicy",
        "tcp-tunnel",
        "responses-api-proxy",
        "stdio-to-uds",
        "cloud",
        "help",
    ] {
        assert!(
            bridge_options(&["-c".into(), "model=example".into(), utility.into()]).is_none(),
            "{utility}"
        );
    }
}

#[test]
fn launcher_forwards_its_directory_unless_overridden_or_bypassed() {
    const TEST: &str = "tests::launcher_forwards_its_directory_unless_overridden_or_bypassed";
    const ARGS_ENV: &str = "YINZ_TEST_CODEX_ARGS";
    const OUTPUT_ENV: &str = "YINZ_TEST_CODEX_OUTPUT";
    if let Ok(socket) = std::env::var("YINZ_TEST_TRUST_SOCKET") {
        assert_eq!(
            std::fs::metadata(Path::new(&socket).parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let listener = std::os::unix::net::UnixListener::bind(socket).unwrap();
        loop {
            let _ = listener.accept().unwrap();
        }
    }
    if std::env::var_os("YINZ_TEST_TRUST_BACKEND").is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let stream = tokio::io::join(tokio::io::stdin(), tokio::io::stderr());
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                let marker = PathBuf::from(std::env::var_os(OUTPUT_ENV).unwrap())
                    .with_extension("registered");
                let mode = std::env::var("YINZ_TEST_TRUST_MODE").unwrap_or_default();
                if mode == "timeout" {
                    std::fs::write(&marker, "started").unwrap();
                    std::future::pending::<()>().await;
                }
                let primary = std::env::var("YINZ_TEST_TRUST_PRIMARY").unwrap();
                let worktree = std::env::var("YINZ_TEST_TRUST_WORKTREE").unwrap();
                while let Some(Ok(message)) = ws.next().await {
                    let request: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
                    let result = match request["method"].as_str().unwrap() {
                        "initialize" => json!({}),
                        "initialized" => continue,
                        "config/read" => {
                            if mode == "malformed" {
                                std::fs::write(&marker, "failed").unwrap();
                                json!({})
                            } else {
                                let mut projects =
                                    json!({primary.as_str():{"trust_level":"trusted"}});
                                if mode == "untrusted" {
                                    projects[&worktree] = json!({"trust_level":"untrusted"});
                                    std::fs::write(&marker, "untrusted").unwrap();
                                } else if matches!(mode.as_str(), "unknown" | "conflict" | "write_error") {
                                    projects = json!({});
                                    std::fs::write(&marker, "unknown").unwrap();
                                }
                                json!({"layers":[{"name":{"type":"user","file":"/tmp/config.toml"},
                                "version":"version-1", "config":{"projects":projects}}]})
                            }
                        },
                        "config/value/write" => {
                            assert_eq!(
                                request["params"]["value"],
                                json!({worktree.as_str():{"trust_level":"trusted"}})
                            );
                            assert_eq!(request["params"]["expectedVersion"], "version-1");
                            assert!(
                                !Path::new(&std::env::var(OUTPUT_ENV).unwrap()).exists(),
                                "UI started before registration"
                            );
                            assert_eq!(request["params"]["keyPath"], "projects");
                            assert_eq!(request["params"]["mergeStrategy"], "upsert");
                            assert_eq!(request["params"]["filePath"], "/tmp/config.toml");
                            if matches!(mode.as_str(), "conflict" | "write_error") {
                                assert_ne!(std::fs::read_to_string(&marker).unwrap(), "write failed", "must not retry approval with a new version");
                                std::fs::write(&marker, "write failed").unwrap();
                                let error = if mode == "conflict" {
                                    json!({"code":-32600,"data":{"config_write_error_code":"configVersionConflict"}})
                                } else {
                                    json!({"code":-32600,"message":"write denied"})
                                };
                                ws.send(Message::text(json!({"id":request["id"],"error":error}).to_string())).await.unwrap();
                                continue;
                            }
                            std::fs::write(&marker, "registered").unwrap();
                            json!({"status":"ok"})
                        },
                        method => panic!("unexpected trust method: {method}"),
                    };
                    if ws
                        .send(Message::text(
                            json!({"id":request["id"],"result":result}).to_string(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        return;
    }
    if let Ok(args) = std::env::var(ARGS_ENV) {
        let args: Vec<String> = serde_json::from_str(&args).unwrap();
        let result = launch(Path::new("../codex"), &args);
        let expected = std::env::var("YINZ_TEST_TRUST_RESULT").unwrap_or_default();
        match expected.as_str() {
            "error" => {
                let error = format!("{:#}", result.unwrap_err());
                assert!(error.contains("Codex"), "{error}");
                if std::env::var("YINZ_TEST_TRUST_MODE").as_deref() == Ok("conflict") {
                    assert!(error.contains("changed while awaiting approval"), "{error}");
                }
                eprintln!("{error}");
            },
            "declined" => assert_eq!(result.unwrap(), 1),
            _ => assert_eq!(result.unwrap(), 23),
        }
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("launcher with spaces");
    std::fs::create_dir(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let worktree_name = "work.tree \"quoted\"";
    for args in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "initial",
        ],
        vec![
            "worktree",
            "add",
            "--quiet",
            "--detach",
            "../work.tree \"quoted\"",
        ],
    ] {
        let result = Command::new("git")
            .args(args)
            .current_dir(&cwd)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let worktree = root.path().join(worktree_name).canonicalize().unwrap();
    assert!(worktree_roots(&cwd).is_none());
    let unregistered = root.path().join("unregistered");
    std::fs::create_dir(&unregistered).unwrap();
    let unregistered = unregistered.canonicalize().unwrap();
    std::fs::write(
        unregistered.join(".git"),
        format!("gitdir: {}\n", cwd.join(".git").display()),
    )
    .unwrap();
    assert!(worktree_roots(&unregistered).is_none());
    std::fs::create_dir(worktree.join("nested")).unwrap();
    assert_eq!(
        worktree_roots(&worktree.join("nested")),
        Some((worktree.clone(), cwd.clone()))
    );
    let alias = root.path().join("worktree-alias");
    std::os::unix::fs::symlink(&worktree, &alias).unwrap();
    assert_eq!(
        worktree_roots(&alias.join("nested")),
        Some((worktree.clone(), cwd.clone()))
    );
    let output = root.path().join("arguments");
    yinz_test_support::write_warm_script(
        &root.path().join("codex"),
        r#"
if [ "$1" = --version ]; then
    printf '%s\n' 'codex-cli 0.162.1'
    exit 0
fi
if [ "$1" = app-server ]; then
    [ "$2" = daemon ] && exit 0
    if [ "$2" = --listen ]; then
        printf '%s\0' "$@" > "$YINZ_TEST_CODEX_OUTPUT.backend"
        export YINZ_TEST_TRUST_SOCKET="${3#unix://}"
        exec "$YINZ_TEST_TRUST_EXE" --exact "$YINZ_TEST_TRUST_TEST" --nocapture
    fi
    if [ "$YINZ_TEST_CODEX_THEME" = 1 ] && [ -z "$2" ]; then
        theme=catppuccin-latte
        [ -f "$YINZ_TEST_CODEX_OUTPUT.registered" ] && theme=dracula
        while IFS= read -r line; do
            case "$line" in
                *'"id":1'*) printf '%s\n' '{"id":1,"result":{}}' ;;
                *'"id":2'*)
                    printf '%s' "$theme" > "$YINZ_TEST_CODEX_OUTPUT.resolved"
                    printf '{"id":2,"result":{"config":{"tui":{"theme":"%s"}},"layers":[]}}\n' "$theme"
                    ;;
            esac
        done
        exit 0
    fi
    export YINZ_TEST_TRUST_BACKEND=1
    exec "$YINZ_TEST_TRUST_EXE" --exact "$YINZ_TEST_TRUST_TEST" --nocapture 2>&1 >/dev/null
fi
if [ "$YINZ_TEST_TRUST_EXPECTED" = 1 ]; then
    [ -f "$YINZ_TEST_CODEX_OUTPUT.registered" ] || exit 99
fi
if [ "$YINZ_TEST_CODEX_THEME" = 1 ]; then
    [ "$(cat "$YINZ_TEST_CODEX_OUTPUT.resolved")" = dracula ] || exit 99
fi
printf '%s\0' "$@" > "$YINZ_TEST_CODEX_OUTPUT"
exit 23
"#,
    );
    // expected_cd: None bypasses the bridge, Some(true) preserves an explicit override.
    let cases: &[(&[&str], bool, Option<bool>)] = &[
        (&[], true, Some(false)),
        (&["prompt with spaces"], true, Some(false)),
        (&["resume", "--last"], true, Some(false)),
        (&["fork", "thread-id"], true, Some(false)),
        (
            &["--sandbox", "read-only", "--ask-for-approval", "on-request"],
            true,
            Some(false),
        ),
        (&["--cd", "../explicit with spaces"], true, Some(true)),
        (&["--cd=../explicit with spaces"], true, Some(true)),
        (&["-C", "../explicit with spaces"], true, Some(true)),
        (&["-C../explicit with spaces"], true, Some(true)),
        (&["-C=../explicit with spaces"], true, Some(true)),
        (&["resume", "--last", "--cd", "../other"], true, Some(true)),
        (&["fork", "thread-id", "-C../other"], true, Some(true)),
        (&["--cd=../other", "resume", "--last"], true, Some(true)),
        (&["--cd", "../unregistered"], true, Some(true)),
        (&["--cd", worktree.to_str().unwrap()], true, Some(true)),
        (&["--cd", "../work.tree \"quoted\""], true, Some(true)),
        (&["--cd=../work.tree \"quoted\""], true, Some(true)),
        (&["-C../work.tree \"quoted\""], true, Some(true)),
        (&["-C=../work.tree \"quoted\""], true, Some(true)),
        (
            &["--cd", "../other", "-C", "../work.tree \"quoted\""],
            true,
            Some(true),
        ),
        (&["--", "--cd=literal prompt"], true, Some(false)),
        (&["--", "-C"], true, Some(false)),
        (&["resume", "--", "--cd"], true, Some(false)),
        (&["--model=--cd", "prompt"], true, None),
        (&["--config", "model='--cd'"], true, Some(false)),
        (
            &[
                "-c",
                r#"projects={"/project space \"quote\".dot"={trust_level="trusted"}}"#,
            ],
            true,
            Some(false),
        ),
        (
            &[r#"--config=projects={"/project.dot"={trust_level="trusted"}}"#],
            true,
            Some(false),
        ),
        (&["prompt"], false, None),
        (&["--cd", "../other"], false, None),
        (&["exec", "prompt"], true, None),
        (&["--remote=unix:///custom"], true, None),
        (&["--no-daemon"], true, None),
        (&["--add-dir", "../other"], true, None),
        (&["--help"], true, None),
        (&["--cd"], true, None),
        (&["--cd="], true, None),
        (&["-C="], true, None),
        (&["--cd", "--", "prompt"], true, None),
    ];
    for launch_cwd in [&cwd, &worktree, &unregistered] {
        for (args, pane, expected_cd) in cases {
            let expected_trust = expected_cd.is_some_and(|has_cd| {
                (!has_cd && launch_cwd == &worktree)
                    || args.iter().any(|arg| arg.ends_with(worktree_name))
            });
            let _ = std::fs::remove_file(&output);
            let _ = std::fs::remove_file(output.with_extension("backend"));
            let _ = std::fs::remove_file(output.with_extension("registered"));
            let result = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST])
                .current_dir(launch_cwd)
                .env(ARGS_ENV, serde_json::to_string(args).unwrap())
                .env(OUTPUT_ENV, &output)
                .env(
                    "YINZ_TEST_TRUST_EXPECTED",
                    if expected_trust { "1" } else { "0" },
                )
                .env("YINZ_TEST_TRUST_PRIMARY", &cwd)
                .env("YINZ_TEST_TRUST_WORKTREE", &worktree)
                .env("YINZ_TEST_TRUST_EXE", std::env::current_exe().unwrap())
                .env("YINZ_TEST_TRUST_TEST", TEST)
                .env("YINZ_PANE_REF", if *pane { "abcdef012345" } else { "" })
                .env_remove("YINZ_REMOTE_PANE_TOKEN")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{args:?}: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
            let actual = std::fs::read_to_string(&output).unwrap();
            let mut actual: Vec<_> = actual.split_terminator('\0').collect();
            if let Some(has_cd) = expected_cd {
                assert_eq!(actual[0], "--remote", "{args:?}");
                assert!(actual[1].starts_with("unix://"), "{args:?}");
                actual.drain(..2);
                if !has_cd {
                    assert_eq!(actual[0], "--cd", "{args:?}");
                    assert_eq!(Path::new(actual[1]), launch_cwd, "{args:?}");
                    actual.drain(..2);
                }
            }
            assert_eq!(&actual, args, "{args:?}");
            let options =
                bridge_options(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>());
            let overrides = options
                .map(|options| options.project_overrides)
                .unwrap_or_default();
            let backend = std::fs::read_to_string(output.with_extension("backend"));
            if overrides.is_empty() {
                assert!(
                    backend.is_err(),
                    "ordinary launch started a private backend"
                );
            } else {
                let backend = backend.unwrap();
                let backend: Vec<_> = backend.split_terminator('\0').collect();
                assert_eq!(&backend[..2], &["app-server", "--listen"]);
                assert_eq!(&backend[3..], &["-c", overrides[0].as_str()]);
            }
            assert_eq!(
                output.with_extension("registered").exists(),
                expected_trust,
                "{args:?}"
            );
        }
    }
    // A terminal launch enables the palette guard. The provider's config/read
    // exposes the project theme only after the trust backend writes registration.
    for args in [vec![], vec!["--cd", worktree.to_str().unwrap()]] {
        let _ = std::fs::remove_file(&output);
        let _ = std::fs::remove_file(output.with_extension("registered"));
        let _ = std::fs::remove_file(output.with_extension("resolved"));
        let pty = nix::pty::openpty(None, None).unwrap();
        let reader = std::thread::spawn(move || {
            let mut master = std::fs::File::from(pty.master);
            let mut bytes = Vec::new();
            let mut buffer = [0; 8192];
            loop {
                match std::io::Read::read(&mut master, &mut buffer) {
                    Ok(0) => break,
                    Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                    Err(error) => panic!("read terminal output: {error}"),
                }
            }
            bytes
        });
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .current_dir(&worktree)
            .env(ARGS_ENV, serde_json::to_string(&args).unwrap())
            .env(OUTPUT_ENV, &output)
            .env("YINZ_TEST_CODEX_THEME", "1")
            .env("YINZ_TEST_TRUST_EXPECTED", "1")
            .env("YINZ_TEST_TRUST_PRIMARY", &cwd)
            .env("YINZ_TEST_TRUST_WORKTREE", &worktree)
            .env("YINZ_TEST_TRUST_EXE", std::env::current_exe().unwrap())
            .env("YINZ_TEST_TRUST_TEST", TEST)
            .env("YINZ_PANE_REF", "abcdef012345")
            .env("CODEX_HOME", root.path().join("codex-home"))
            .env_remove("YINZ_REMOTE_PANE_TOKEN")
            .env_remove("YINZ_BUBBLEWRAP")
            .stdout(std::fs::File::from(pty.slave))
            .output()
            .unwrap();
        let bytes = reader.join().unwrap();
        let stdout = String::from_utf8_lossy(&bytes);
        assert!(result.status.success(), "{stdout}\n{result:?}");
        assert!(stdout.contains("1 passed"), "{stdout}");
        assert!(stdout.contains("yinz-agent-theme;"), "{stdout}");
        assert_eq!(
            std::fs::read_to_string(output.with_extension("resolved")).unwrap(),
            "dracula"
        );
    }
    for (mode, input, expected) in [
        ("untrusted", "y\n", "approved"),
        ("unknown", "YES\n", "approved"),
        ("unknown", "YeS\n", "approved"),
        ("unknown", "n\n", "declined"),
        ("untrusted", "\n", "declined"),
        ("unknown", "", "declined"),
        ("unknown", "yes please\n", "declined"),
        ("unknown", "\u{3}\n", "declined"),
        ("unknown", "interrupt", "declined"),
        ("malformed", "y\n", "error"),
        ("timeout", "y\n", "error"),
        ("write_error", "yes\n", "error"),
        ("conflict", "y\n", "error"),
    ] {
        let _ = std::fs::remove_file(&output);
        let _ = std::fs::remove_file(output.with_extension("registered"));
        let args = [
            "--cd",
            worktree.to_str().unwrap(),
            "--model",
            "example",
            "resume",
            "--last",
        ];
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .current_dir(&cwd)
            .env(ARGS_ENV, serde_json::to_string(&args).unwrap())
            .env(OUTPUT_ENV, &output)
            .env("YINZ_TEST_TRUST_EXPECTED", "1")
            .env("YINZ_TEST_TRUST_MODE", mode)
            .env("YINZ_TEST_TRUST_RESULT", expected)
            .env("YINZ_TEST_TRUST_PRIMARY", &cwd)
            .env("YINZ_TEST_TRUST_WORKTREE", &worktree)
            .env("YINZ_TEST_TRUST_EXE", std::env::current_exe().unwrap())
            .env("YINZ_TEST_TRUST_TEST", TEST)
            .env("YINZ_PANE_REF", "abcdef012345")
            .env_remove("YINZ_REMOTE_PANE_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if input == "interrupt" {
            let mut prompt = Vec::new();
            let mut byte = [0];
            while !prompt.ends_with(b"Trust this worktree? [y/N] ") {
                child
                    .stdout
                    .as_mut()
                    .unwrap()
                    .read_exact(&mut byte)
                    .unwrap();
                prompt.extend(byte);
            }
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(child.id() as i32),
                nix::sys::signal::Signal::SIGINT,
            )
            .unwrap();
            assert_eq!(child.wait().unwrap().signal(), Some(nix::libc::SIGINT));
            assert!(!output.exists(), "interruption must not launch Codex");
            continue;
        }
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let result = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(
            result.status.success(),
            "{mode}, {input:?}: {stdout} {}",
            String::from_utf8_lossy(&result.stderr)
        );
        if !matches!(mode, "malformed" | "timeout") {
            assert!(stdout.contains("Trust this worktree? [y/N]"), "{stdout}");
            assert!(stdout.contains(worktree.to_str().unwrap()), "{stdout}");
            assert!(stdout.contains("persists trust"), "{stdout}");
        }
        if expected == "approved" {
            let actual = std::fs::read_to_string(&output).unwrap();
            let actual = actual.split_terminator('\0').collect::<Vec<_>>();
            assert_eq!(actual[0], "--remote");
            assert!(actual[1].starts_with("unix://"));
            assert_eq!(&actual[2..], args);
            assert_eq!(
                std::fs::read_to_string(output.with_extension("registered")).unwrap(),
                "registered"
            );
        } else {
            assert!(!output.exists(), "{mode}, {input:?} must not launch Codex");
        }
    }
}

#[test]
fn launcher_keeps_the_ui_alive_across_backend_connection_failures() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const TEST: &str = "tests::launcher_keeps_the_ui_alive_across_backend_connection_failures";
    const ROLE_ENV: &str = "YINZ_TEST_CODEX_RECONNECT_ROLE";
    const ROOT_ENV: &str = "YINZ_TEST_CODEX_RECONNECT_ROOT";
    const REQUEST: &str =
        r#"{"id":0,"method":"turn/start","params":{"threadId":"reconnect-thread"}}"#;
    const REPLY: &str = r#"{"id":0,"result":true}"#;
    let role = std::env::var(ROLE_ENV).unwrap_or_default();
    if role == "launcher" {
        let program = PathBuf::from(std::env::var_os(ROOT_ENV).unwrap()).join("codex");
        assert_eq!(launch(&program, &[]).unwrap(), 23);
        return;
    }

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                match role.as_str() {
                    "ui" => {
                        let endpoint = std::env::var("YINZ_TEST_CODEX_RECONNECT_SOCKET").unwrap();
                        let socket = Path::new(endpoint.strip_prefix("unix://").unwrap());
                        let binding = socket.parent().unwrap().join("binding.json");
                        let mut selected = None;
                        for attempt in 0..4 {
                            let stream = tokio::net::UnixStream::connect(socket).await.unwrap();
                            let (mut ws, _) =
                                tokio_tungstenite::client_async("ws://localhost", stream)
                                    .await
                                    .unwrap();
                            if matches!(attempt, 1 | 3) {
                                ws.send(Message::text(REQUEST)).await.unwrap();
                                assert_eq!(
                                    ws.next().await.unwrap().unwrap(),
                                    Message::text(REPLY)
                                );
                                selected.get_or_insert_with(|| {
                                    let bytes = std::fs::read(&binding).unwrap();
                                    let value: Value = serde_json::from_slice(&bytes).unwrap();
                                    assert_eq!(value["thread_id"], "reconnect-thread");
                                    bytes
                                });
                            }
                            if attempt == 3 {
                                ws.close(None).await.unwrap();
                                assert!(matches!(
                                    ws.next().await.unwrap().unwrap(),
                                    Message::Close(_)
                                ));
                            } else {
                                assert!(ws.next().await.unwrap().is_err());
                            }
                            if let Some(selected) = &selected {
                                assert_eq!(&std::fs::read(&binding).unwrap(), selected);
                            } else {
                                // Startup can fail before a conversation is selected. The
                                // launcher must still keep this unbound, Spawned UI alive.
                                assert!(!binding.exists());
                            }
                        }
                    },
                    "backend" => {
                        let counter = PathBuf::from(std::env::var_os(ROOT_ENV).unwrap())
                            .join("connections");
                        let attempt: usize = std::fs::read_to_string(&counter)
                            .unwrap()
                            .parse()
                            .unwrap();
                        std::fs::write(&counter, (attempt + 1).to_string()).unwrap();
                        // The script routes stderr to the proxy pipe, keeping libtest output out.
                        let mut stream = tokio::io::join(tokio::io::stdin(), tokio::io::stderr());
                        if matches!(attempt, 0 | 2) {
                            let mut request = Vec::new();
                            while !request.ends_with(b"\r\n\r\n") {
                                request.push(stream.read_u8().await.unwrap());
                            }
                            stream
                                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                                .await
                                .unwrap();
                            stream.flush().await.unwrap();
                        } else {
                            assert!(matches!(attempt, 1 | 3));
                            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                            assert_eq!(
                                ws.next().await.unwrap().unwrap(),
                                Message::text(REQUEST)
                            );
                            ws.send(Message::text(REPLY)).await.unwrap();
                            if attempt == 3 {
                                assert!(matches!(
                                    ws.next().await.unwrap().unwrap(),
                                    Message::Close(_)
                                ));
                                ws.flush().await.unwrap();
                            }
                            // The first connected backend exits without a WebSocket close.
                        }
                    },
                    "" => {
                        let root = tempfile::tempdir().unwrap();
                        std::fs::write(root.path().join("connections"), "0").unwrap();
                        yinz_test_support::write_warm_script(
                            &root.path().join("codex"),
                            r#"
if [ "$1" = app-server ]; then
    [ "$2" = daemon ] && exit 0
    export YINZ_TEST_CODEX_RECONNECT_ROLE=backend
    exec "$YINZ_TEST_CODEX_RECONNECT_EXE" --exact "$YINZ_TEST_CODEX_RECONNECT_TEST" --nocapture 2>&1 >/dev/null
fi
export YINZ_TEST_CODEX_RECONNECT_ROLE=ui
export YINZ_TEST_CODEX_RECONNECT_SOCKET="$2"
exec "$YINZ_TEST_CODEX_RECONNECT_EXE" --exact "$YINZ_TEST_CODEX_RECONNECT_TEST" --nocapture
"#,
                        );
                        let executable = std::env::current_exe().unwrap();
                        let output = tokio::process::Command::new(&executable)
                            .args(["--exact", TEST, "--nocapture"])
                            .current_dir(root.path())
                            .env(ROLE_ENV, "launcher")
                            .env(ROOT_ENV, root.path())
                            .env("YINZ_TEST_CODEX_RECONNECT_EXE", &executable)
                            .env("YINZ_TEST_CODEX_RECONNECT_TEST", TEST)
                            .env("YINZ_PANE_REF", "abcdef012345")
                            .env_remove("YINZ_REMOTE_PANE_TOKEN")
                            .kill_on_drop(true)
                            .output()
                            .await
                            .unwrap();
                        assert!(
                            output.status.success(),
                            "{}\n{}",
                            String::from_utf8_lossy(&output.stdout),
                            String::from_utf8_lossy(&output.stderr)
                        );
                        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
                        assert_eq!(
                            std::fs::read_to_string(root.path().join("connections")).unwrap(),
                            "4"
                        );
                    },
                    _ => panic!("unexpected mock Codex role: {role}"),
                }
            })
            .await
            .expect("mock Codex reconnect scenario timed out");
        });
    if role == "ui" {
        std::process::exit(23);
    }
}

#[test]
fn launcher_reports_an_unresolvable_working_directory() {
    const TEST: &str = "tests::launcher_reports_an_unresolvable_working_directory";
    const CHILD_ENV: &str = "YINZ_TEST_CODEX_DELETED_CWD";
    if std::env::var_os(CHILD_ENV).is_some() {
        std::fs::remove_dir(std::env::current_dir().unwrap()).unwrap();
        let error = launch(Path::new("/usr/bin/true"), &[]).unwrap_err();
        assert!(error
            .to_string()
            .contains("resolve Codex launcher working directory"));
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST])
        .env(CHILD_ENV, "1")
        .env("YINZ_PANE_REF", "abcdef012345")
        .env_remove("YINZ_REMOTE_PANE_TOKEN")
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

async fn assert_bridge_child_stopped(pid: u32) {
    let pid = nix::unistd::Pid::from_raw(pid as i32);
    let status = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || nix::sys::wait::waitpid(pid, None)),
    )
    .await;
    if status.is_err() {
        // Release the waiter even when the regression leaves its child running.
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL).unwrap();
        panic!("bridge child {pid} survived cancellation");
    }
    let status = status.unwrap().unwrap();
    assert!(
        matches!(
            status,
            Ok(nix::sys::wait::WaitStatus::Signaled(
                _,
                nix::sys::signal::Signal::SIGKILL,
                _
            )) | Err(nix::errno::Errno::ECHILD)
        ),
        "{status:?}"
    );
}

#[tokio::test]
async fn reconnects_replace_only_after_handshake_and_cancel_owned_children() {
    use tokio::io::AsyncReadExt;

    const TEST: &str = "tests::reconnects_replace_only_after_handshake_and_cancel_owned_children";
    if let Ok(socket) = std::env::var("YINZ_TEST_CODEX_PEER_SOCKET") {
        let mut peer = tokio::net::UnixStream::connect(socket).await.unwrap();
        peer.write_u32(std::process::id()).await.unwrap();
        if std::env::var("YINZ_TEST_CODEX_PEER_ROLE").unwrap() == "backend" {
            // The script keeps libtest output out of the WebSocket pipe.
            let mut stream = tokio::io::join(tokio::io::stdin(), tokio::io::stderr());
            tokio::io::copy_bidirectional(&mut peer, &mut stream)
                .await
                .unwrap();
        } else {
            let mut hook = Vec::new();
            tokio::io::stdin().read_to_end(&mut hook).await.unwrap();
            peer.write_all(&hook).await.unwrap();
            peer.shutdown().await.unwrap();
            // Hold the report open after receiving EOF on stdin, until killed by its owner.
            peer.read_u8().await.unwrap();
        }
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("ui.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let peer_socket = root.path().join("peer.sock");
    let peers = tokio::net::UnixListener::bind(&peer_socket).unwrap();
    let program = root.path().join("backend");
    let reporter = root.path().join("reporter");
    for (path, role) in [(&program, "backend"), (&reporter, "reporter")] {
        yinz_test_support::write_warm_script(
            path,
            &format!(
                "export YINZ_TEST_CODEX_PEER_SOCKET='{}'\nexport YINZ_TEST_CODEX_PEER_ROLE={role}\nexec '{}' --exact '{TEST}' --nocapture 2>&1 >/dev/null\n",
                peer_socket.display(),
                std::env::current_exe().unwrap().display(),
            ),
        );
    }
    let mut registration = registration(root.path(), "aaaaaaaaaaaa");
    registration.reporter = Some(reporter);

    async fn backend(
        peers: &tokio::net::UnixListener,
    ) -> (
        tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
        u32,
    ) {
        let (mut stream, _) = peers.accept().await.unwrap();
        let pid = stream.read_u32().await.unwrap();
        (tokio_tungstenite::accept_async(stream).await.unwrap(), pid)
    }

    async fn exchange(
        ui: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
        backend: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    ) {
        let request =
            Message::text(r#"{"id":0,"method":"turn/start","params":{"threadId":"selected"}}"#);
        ui.send(request.clone()).await.unwrap();
        assert_eq!(backend.next().await.unwrap().unwrap(), request);
        let reply = Message::text(r#"{"id":0,"result":true}"#);
        backend.send(reply.clone()).await.unwrap();
        assert_eq!(ui.next().await.unwrap().unwrap(), reply);
    }

    async fn stalled_report(
        ui: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
        backend: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
        peers: &tokio::net::UnixListener,
    ) -> (tokio::net::UnixStream, u32) {
        let request = Message::text(r#"{"id":1,"method":"thread/resume"}"#);
        ui.send(request.clone()).await.unwrap();
        assert_eq!(backend.next().await.unwrap().unwrap(), request);
        backend
            .send(Message::text(
                r#"{"id":1,"result":{"thread":{"id":"selected"}}}"#,
            ))
            .await
            .unwrap();
        let (mut stream, _) = peers.accept().await.unwrap();
        let pid = stream.read_u32().await.unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).await.unwrap();
        let hook: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(hook["hook_event_name"], "SessionStart");
        assert_eq!(hook["session_id"], "selected");
        (stream, pid)
    }

    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut serve = Box::pin(connections(listener, &program, &registration));
        let scenario = async {
            let mut incomplete = tokio::net::UnixStream::connect(&socket).await.unwrap();
            let (mut ui, _) = tokio_tungstenite::client_async(
                "ws://localhost",
                tokio::net::UnixStream::connect(&socket).await.unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(incomplete.read(&mut [0]).await.unwrap(), 0);
            let (mut daemon, mut daemon_pid) = backend(&peers).await;

            let mut invalid = tokio::net::UnixStream::connect(&socket).await.unwrap();
            invalid.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
            assert_eq!(invalid.read(&mut [0]).await.unwrap(), 0);
            exchange(&mut ui, &mut daemon).await;

            let started = std::time::Instant::now();
            let mut pending = tokio::net::UnixStream::connect(&socket).await.unwrap();
            exchange(&mut ui, &mut daemon).await;
            assert_eq!(pending.read(&mut [0]).await.unwrap(), 0);
            assert!(started.elapsed() >= std::time::Duration::from_secs(10));
            exchange(&mut ui, &mut daemon).await;

            for attempt in 0..3 {
                let report = if attempt == 0 {
                    None
                } else {
                    Some(stalled_report(&mut ui, &mut daemon, &peers).await)
                };
                let binding = std::fs::read(registration.dir.path().join("binding.json")).unwrap();
                let (replacement, _) = tokio_tungstenite::client_async(
                    "ws://localhost",
                    tokio::net::UnixStream::connect(&socket).await.unwrap(),
                )
                .await
                .unwrap();
                let (next_daemon, next_pid) = backend(&peers).await;
                assert!(ui.next().await.unwrap().is_err());
                assert!(daemon.next().await.unwrap().is_err());
                assert_bridge_child_stopped(daemon_pid).await;
                if let Some((_stream, pid)) = report {
                    assert_bridge_child_stopped(pid).await;
                }
                assert_eq!(
                    std::fs::read(registration.dir.path().join("binding.json")).unwrap(),
                    binding,
                    "reconnect must retain the selected conversation"
                );
                ui = replacement;
                daemon = next_daemon;
                daemon_pid = next_pid;
                exchange(&mut ui, &mut daemon).await;
            }
            let report = stalled_report(&mut ui, &mut daemon, &peers).await;
            let mut displaced = tokio::net::UnixStream::connect(&socket).await.unwrap();
            let pending = tokio::net::UnixStream::connect(&socket).await.unwrap();
            assert_eq!(displaced.read(&mut [0]).await.unwrap(), 0);
            (ui, daemon, daemon_pid, report, pending)
        };
        let (mut ui, mut daemon, daemon_pid, (_report_stream, report_pid), mut pending) =
            tokio::select! {
                result = &mut serve => panic!("connection loop stopped: {result:?}"),
                peers = scenario => peers,
            };
        // Launcher exit drops this same future while the UI and its report are still connected.
        drop(serve);
        assert!(ui.next().await.unwrap().is_err());
        assert!(daemon.next().await.unwrap().is_err());
        assert_eq!(pending.read(&mut [0]).await.unwrap(), 0);
        assert_bridge_child_stopped(daemon_pid).await;
        assert_bridge_child_stopped(report_pid).await;
    })
    .await
    .expect("Codex reconnect scenario timed out");
}

#[test]
fn routed_native_hooks_preserve_small_payloads_and_bound_large_ones() {
    const TEST: &str = "tests::routed_native_hooks_preserve_small_payloads_and_bound_large_ones";
    if let Ok(reporter) = std::env::var("YINZ_TEST_CODEX_REPORTER") {
        report(Path::new(&reporter)).unwrap();
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let delivered = root.path().join("delivered");
    let reporter = root.path().join("reporter");
    yinz_test_support::write_warm_script(
        &reporter,
        &format!(
            "[ \"$*\" = '--pty-broker report codex' ] || exit 1\ncat > '{}'\n",
            delivered.display(),
        ),
    );
    let session = uuid::Uuid::new_v4().to_string();
    let registration = Registration::new(
        &directory(),
        HashMap::from([("YINZ_REMOTE_PANE_TOKEN".into(), "remote-token".into())]),
    )
    .unwrap();
    registration.select(&session).unwrap();
    let forward = |raw: &str| {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env("YINZ_TEST_CODEX_REPORTER", &reporter)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(raw.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::read_to_string(&delivered).unwrap()
    };

    let prompt = "review\n".repeat(100 * 1024);
    let submit = json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": session,
        "turn_id": "turn-1",
        "prompt": prompt,
    })
    .to_string();
    assert!(submit.len() > yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
    let forwarded = forward(&submit);
    assert!(forwarded.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
    let hook: yinz_types::agent::HookEvent = serde_json::from_str(&forwarded).unwrap();
    assert_eq!(hook.session_id, session);
    assert_eq!(hook.turn_id.as_deref(), Some("turn-1"));
    assert_eq!(
        hook.prompt_sha256.as_deref(),
        Some(yinz_types::agent::control::prompt_sha256(&prompt)).as_deref()
    );
    let event = yinz_types::agent::AgentEvent::from_hook(
        "codex",
        yinz_types::internal_ids::TerminalId::new(7),
        &hook,
        forwarded,
    );
    let mut observation =
        yinz_types::agent::control::PromptObservation::new("codex", &session, &prompt);
    assert_eq!(
        observation.observe(&event, Some(&hook)),
        Some(yinz_types::agent::control::AgentOutcome::Submitted)
    );

    let stop = json!({
        "hook_event_name": "Stop",
        "session_id": session,
        "turn_id": "turn-1",
        "last_assistant_message": "done\n".repeat(500 * 1024),
    })
    .to_string();
    assert!(stop.len() > yinz_types::agent::NATIVE_HOOK_MAX_BYTES);
    let forwarded = forward(&stop);
    assert!(forwarded.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
    let hook: yinz_types::agent::HookEvent = serde_json::from_str(&forwarded).unwrap();
    assert_eq!(hook.hook_event_name, "Stop");
    assert_eq!(hook.session_id, session);
    assert_eq!(hook.turn_id.as_deref(), Some("turn-1"));
    assert_eq!(
        yinz_types::agent::hook_state(&hook),
        Some(yinz_types::agent::AgentState::Idle)
    );

    let payload = json!({
        "text": format!("tool preview {}", "雪".repeat(200000)),
        "password": "must-not-persist",
    });
    for (response, error, status) in [
        (
            json!({"exit_code": 1, "output": payload}),
            Value::Null,
            "failed",
        ),
        (
            json!({"exit_code": 0, "output": payload}),
            Value::Null,
            "succeeded",
        ),
        (Value::Null, payload, "failed"),
    ] {
        let tool = json!({
            "hook_event_name": "PostToolUse",
            "session_id": session,
            "turn_id": "turn-1",
            "tool_use_id": "call-1",
            "tool_name": "shell",
            "agent_id": "worker",
            "parent_tool_use_id": format!("PASSWORD=hidden {}", "parent".repeat(200)),
            "tool_response": response,
            "error": error,
        })
        .to_string();
        assert!(tool.len() > yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
        let (prepared, _) =
            yinz_runtime::agent::hooks::parse_hook_event("codex", tool.clone()).unwrap();
        let expected = prepared.tool_capture.unwrap();
        let forwarded = forward(&tool);
        assert!(forwarded.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
        let (hook, raw) = yinz_runtime::agent::hooks::parse_hook_event("codex", forwarded).unwrap();
        assert_eq!(hook.session_id, session);
        assert_eq!(hook.turn_id.as_deref(), Some("turn-1"));
        assert!(hook.tool_response.is_none());
        assert!(hook.error.is_none());
        let capture = hook.tool_capture.unwrap();
        assert_eq!(capture.status, status);
        assert!(capture.identity.is_some());
        assert_eq!(capture.identity, expected.identity);
        assert_eq!(capture.output_unavailable, response.is_null());
        let preview = capture.output.as_ref().or(capture.error.as_ref()).unwrap();
        assert!(preview.text.contains("tool preview"));
        assert!(preview.truncated && preview.redacted);
        assert!(preview.original_bytes > yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
        assert!(!capture.text().contains("must-not-persist"));
        assert!(capture.metadata_truncated && capture.metadata_redacted);
        assert_eq!(capture, expected);
        let serialized: yinz_types::agent::HookEvent = serde_json::from_str(&raw).unwrap();
        assert_eq!(serialized.tool_capture, Some(expected));
    }

    let small = format!(
        " {{ \"hook_event_name\": \"Stop\", \"session_id\": \"{session}\", \"turn_id\": \"turn-1\", \"unknown\": true }}\n"
    );
    assert_eq!(forward(&small), small);
    let at_cap = format!(
        "{small}{}",
        " ".repeat(yinz_types::agent::HOOK_BROADCAST_MAX_BYTES - small.len())
    );
    assert_eq!(forward(&at_cap), at_cap);
    let malformed = format!(
        "{{\"padding\": \"{}",
        "x".repeat(yinz_types::agent::HOOK_BROADCAST_MAX_BYTES)
    );
    assert!(prune_for_forward(malformed).is_err());
}

#[test]
fn registry_and_a_session_named_codex_coexist() {
    const CHILD_ENV: &str = "YINZ_TEST_CODEX_REGISTRY";
    const THREAD_ENV: &str = "YINZ_TEST_CODEX_THREAD";
    const SOCKETS_ENV: &str = "YINZ_TEST_CODEX_SOCKETS";
    const TEST: &str = "tests::registry_and_a_session_named_codex_coexist";
    if std::env::var_os(CHILD_ENV).is_none() {
        for source in ["fallback", "runtime", "override"] {
            let root = tempfile::tempdir().unwrap();
            let mut launcher = Command::new(std::env::current_exe().unwrap());
            launcher
                .args(["--exact", TEST])
                .env(CHILD_ENV, source)
                .env("HOME", root.path())
                .env("TMPDIR", root.path())
                .env_remove("YINZ_SOCKET_DIR")
                .env_remove("XDG_RUNTIME_DIR");
            if source != "fallback" {
                launcher.env("XDG_RUNTIME_DIR", root.path().join("runtime"));
            }
            if source == "override" {
                launcher.env("YINZ_SOCKET_DIR", root.path().join("sockets"));
            }
            let output = launcher.output().unwrap();
            assert!(
                output.status.success(),
                "{source}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
        return;
    }
    if std::env::var(CHILD_ENV).unwrap() == "lookup" {
        let binding = binding_for(&directory(), &std::env::var(THREAD_ENV).unwrap())
            .unwrap()
            .expect("the daemon must find the launcher's live binding");
        let sockets = PathBuf::from(std::env::var(SOCKETS_ENV).unwrap());
        assert_eq!(
            Path::new(&binding.env["YINZ_SOCKET_DIR"])
                .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
                .canonicalize()
                .unwrap(),
            sockets.canonicalize().unwrap()
        );
        if cfg!(target_os = "linux") && std::env::var_os("XDG_RUNTIME_DIR").is_some() {
            assert_ne!(&*yinz_runtime::consts::YINZ_SOCK_DIR, &sockets);
        }
        return;
    }
    let sockets = &*yinz_runtime::consts::YINZ_SOCK_DIR;
    std::fs::create_dir_all(sockets).unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(sockets.join("codex")).unwrap();
    let registration = registration(&directory(), "aaaaaaaaaaaa");
    let thread = uuid::Uuid::new_v4().to_string();
    registration.select(&thread).unwrap();
    for runtime in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut daemon = Command::new(std::env::current_exe().unwrap());
        daemon
            .args(["--exact", TEST])
            .env(CHILD_ENV, "lookup")
            .env(THREAD_ENV, &thread)
            .env(SOCKETS_ENV, sockets)
            .env("TMPDIR", root.path())
            .env_remove("YINZ_SOCKET_DIR")
            .env_remove("XDG_RUNTIME_DIR");
        if runtime {
            daemon.env("XDG_RUNTIME_DIR", root.path().join("runtime"));
        }
        let output = daemon.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
    assert_eq!(
        std::fs::metadata(directory()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(std::os::unix::net::UnixStream::connect(sockets.join("codex")).is_ok());
}

fn registration(root: &Path, pane: &str) -> Registration {
    Registration::new(root, HashMap::from([("YINZ_PANE_REF".into(), pane.into())])).unwrap()
}

#[test]
fn daemon_conversations_follow_their_uis_not_the_daemons_pane() {
    let root = tempfile::tempdir().unwrap();
    let first = registration(root.path(), "aaaaaaaaaaaa");
    let second = registration(root.path(), "bbbbbbbbbbbb");
    let mut connection = Conversations::default();
    connection
        .request(&json!({"id":1,"method":"thread/start"}), &first)
        .unwrap();
    connection
        .response(
            &json!({"id":1,"method":"some/server/request","params":{}}),
            &first,
        )
        .unwrap();
    // An unrelated thread/started notification (eg a background worker) cannot claim the UI.
    connection
        .response(
            &json!({"method":"thread/started","params":{"thread":{"id":"worker"}}}),
            &first,
        )
        .unwrap();
    assert!(binding_for(root.path(), "worker").unwrap().is_none());
    connection
        .response(
            &json!({"id":1,"result":{"thread":{"id":"thread-a"}}}),
            &first,
        )
        .unwrap();
    second.select("thread-b").unwrap();
    assert_eq!(
        binding_for(root.path(), "thread-a").unwrap().unwrap().env["YINZ_PANE_REF"],
        "aaaaaaaaaaaa"
    );
    assert_eq!(
        binding_for(root.path(), "thread-b").unwrap().unwrap().env["YINZ_PANE_REF"],
        "bbbbbbbbbbbb"
    );

    for method in ["thread/resume", "thread/fork"] {
        connection
            .request(&json!({"id":"next","method":method}), &first)
            .unwrap();
        connection
            .response(
                &json!({"id":"next","error":{"message":"not found"}}),
                &first,
            )
            .unwrap();
        assert!(binding_for(root.path(), "thread-a").unwrap().is_some());
        connection
            .request(&json!({"id":"next","method":method}), &first)
            .unwrap();
        connection
            .response(
                &json!({"id":"next","result":{"thread":{"id":"selected"}}}),
                &first,
            )
            .unwrap();
        assert!(binding_for(root.path(), "thread-a").unwrap().is_none());
        assert!(binding_for(root.path(), "selected").unwrap().is_some());
        first.select("thread-a").unwrap();
    }
    connection
        .request(
            &json!({"id":3,"method":"turn/start","params":{"threadId":"selected"}}),
            &first,
        )
        .unwrap();
    assert!(binding_for(root.path(), "thread-a").unwrap().is_none());
    assert!(binding_for(root.path(), "selected").unwrap().is_some());

    first._lease.unlock().unwrap();
    assert!(
        binding_for(root.path(), "selected").unwrap().is_none(),
        "a crashed UI's files cannot route hooks"
    );
    drop(second);
    assert!(binding_for(root.path(), "thread-b").unwrap().is_none());
}

#[test]
fn hidden_helper_threads_never_claim_the_pane_or_announce_themselves() {
    let root = tempfile::tempdir().unwrap();
    let ui = registration(root.path(), "aaaaaaaaaaaa");
    let mut connection = Conversations::default();
    connection
        .request(
            &json!({"id":"startup","method":"thread/start","params":{"cwd":"/work"}}),
            &ui,
        )
        .unwrap();
    let announced = connection
        .response(
            &json!({"id":"startup","result":{"thread":{"id":"real","cwd":"/work","ephemeral":false}}}),
            &ui,
        )
        .unwrap();
    assert_eq!(
        announced,
        vec![json!({
            "hook_event_name":"SessionStart",
            "session_id":"real",
            "source":"startup",
            "cwd":"/work",
            "_yinz_codex_hooks_ready":false,
        })]
    );
    // The TUI titles a conversation from a hidden thread it starts on the first prompt. Either
    // the request or the reply may be what marks it ephemeral.
    for (id, params, thread) in [
        (
            "temporary-structured-a",
            json!({"ephemeral":true}),
            json!({"id":"title-a"}),
        ),
        (
            "temporary-structured-b",
            json!({}),
            json!({"id":"title-b","ephemeral":true}),
        ),
    ] {
        connection
            .request(
                &json!({"id":"turn","method":"turn/start","params":{"threadId":"real"}}),
                &ui,
            )
            .unwrap();
        connection
            .request(
                &json!({"id":id,"method":"thread/start","params":params}),
                &ui,
            )
            .unwrap();
        let title = thread["id"].as_str().unwrap().to_owned();
        assert_eq!(
            connection
                .response(&json!({"id":id,"result":{"thread":thread}}), &ui)
                .unwrap(),
            Vec::<Value>::new()
        );
        connection
            .request(
                &json!({"id":"title","method":"turn/start","params":{"threadId":title}}),
                &ui,
            )
            .unwrap();
        assert!(binding_for(root.path(), &title).unwrap().is_none());
        assert!(binding_for(root.path(), "real").unwrap().is_some());
    }

    for (method, source) in [("thread/resume", "resume"), ("thread/fork", "fork")] {
        connection
            .request(&json!({"id":"next","method":method}), &ui)
            .unwrap();
        let announced = connection
            .response(
                &json!({"id":"next","result":{"thread":{"id":"other"}}}),
                &ui,
            )
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(announced["source"], source);
        assert!(announced.get("cwd").is_none());
        assert!(binding_for(root.path(), "other").unwrap().is_some());
    }
}

#[test]
fn a_new_thread_replacing_the_selected_one_is_reported_as_a_clear() {
    let root = tempfile::tempdir().unwrap();
    let ui = registration(root.path(), "aaaaaaaaaaaa");
    let mut connection = Conversations::default();
    let mut start = |id: &str, thread: Value| {
        connection
            .request(&json!({"id":id,"method":"thread/start"}), &ui)
            .unwrap();
        connection
            .response(&json!({"id":id,"result":{"thread":thread}}), &ui)
            .unwrap()
    };
    let first = start("1", json!({"id":"first","status":{"type":"idle"}}));
    assert_eq!(first.len(), 1);
    assert_eq!(first[0]["source"], "startup");
    // The title generator's hidden thread is not a replacement.
    assert!(start("2", json!({"id":"title","ephemeral":true})).is_empty());
    // `/new`.
    let replaced = start(
        "3",
        json!({"id":"second","cwd":"/work","status":{"type":"idle"}}),
    );
    assert_eq!(
        replaced,
        vec![
            json!({"hook_event_name":"SessionEnd","session_id":"first","reason":"clear"}),
            json!({
                "hook_event_name":"SessionStart",
                "session_id":"second",
                "source":"clear",
                "cwd":"/work",
                "_yinz_codex_hooks_ready":true,
            }),
        ]
    );
    assert!(binding_for(root.path(), "first").unwrap().is_none());
    assert!(binding_for(root.path(), "second").unwrap().is_some());
}

#[test]
fn selected_conversations_carry_the_panes_environment_to_tool_commands() {
    let env = HashMap::from([
        ("YINZ_PANE_ID".to_owned(), "4".to_owned()),
        ("YINZ_SESSION_NAME".to_owned(), "calm-otter".to_owned()),
    ]);
    for method in ["thread/start", "thread/resume", "thread/fork"] {
        for params in [json!({"cwd":"/work"}), json!({"cwd":"/work","config":null})] {
            let mut request = json!({"id":1,"method":method,"params":params});
            assert!(inject_pane_env(&mut request, &env));
            assert_eq!(
                request["params"],
                json!({"cwd":"/work","config":{
                    "shell_environment_policy.set.YINZ_PANE_ID":"4",
                    "shell_environment_policy.set.YINZ_SESSION_NAME":"calm-otter",
                }})
            );
        }
    }

    // A nested table from the UI keeps its own entries; the pane's identity wins its keys.
    let mut request = json!({"id":1,"method":"thread/start","params":{"config":{
        "model":"m",
        "shell_environment_policy":{"inherit":"core","set":{"KEEP":"1","YINZ_PANE_ID":"9"}},
    }}});
    assert!(inject_pane_env(&mut request, &env));
    assert_eq!(
        request["params"]["config"],
        json!({
            "model":"m",
            "shell_environment_policy":{"inherit":"core","set":{
                "KEEP":"1","YINZ_PANE_ID":"4","YINZ_SESSION_NAME":"calm-otter",
            }},
        })
    );

    for untouched in [
        json!({"id":1,"method":"thread/start","params":{"ephemeral":true}}),
        json!({"id":1,"method":"turn/start","params":{"threadId":"t"}}),
        json!({"id":1,"method":"thread/start"}),
        json!({"id":1,"method":"thread/start","params":{"config":"odd"}}),
    ] {
        let mut request = untouched.clone();
        assert!(!inject_pane_env(&mut request, &env));
        assert_eq!(request, untouched);
    }
    let mut request = json!({"id":1,"method":"thread/start","params":{}});
    assert!(!inject_pane_env(&mut request, &HashMap::new()));
}

#[tokio::test]
async fn bridge_registers_before_delivering_replies_and_preserves_the_protocol() {
    for (close_from_ui, wait_for_ack) in
        [(true, true), (false, true), (true, false), (false, false)]
    {
        let root = tempfile::tempdir().unwrap();
        let registration = registration(root.path(), "aaaaaaaaaaaa");
        let (ui_stream, relay_stream) = tokio::net::UnixStream::pair().unwrap();
        let (backend_stream, server_stream) = tokio::io::duplex(4096);
        let requests = [
            r#"{ "id":0,"method":"initialize","params":{"clientInfo":{"name":"test","version":"1"}},"extra":true }"#,
            r#"{"method":"initialized","extra":[1,2]}"#,
            r#"{"id":1,"method":"thread/start","params":{"cwd":"/same-directory"},"extra":"preserved"}"#,
        ];
        let replies = [
            r#"{ "id":0,"result":{"userAgent":"test"},"extra":true }"#,
            r#"{"method":"turn/started","params":{"threadId":"worker"},"extra":[1,2]}"#,
            r#"{"id":1,"result":{"thread":{"id":"thread-a"},"extra":"preserved"}}"#,
        ];
        let serve = async {
            let ui = tokio_tungstenite::accept_async(relay_stream).await.unwrap();
            relay(
                ui,
                backend_stream,
                &registration,
                Path::new("/usr/bin/true"),
            )
            .await
            .unwrap();
        };
        let backend = async {
            let mut ws = tokio_tungstenite::accept_async(server_stream)
                .await
                .unwrap();
            ws.send(Message::Ping(b"backend".to_vec().into()))
                .await
                .unwrap();
            // The UI request can arrive before the relay processes the backend ping.
            let mut pong_received = false;
            for (request, reply) in requests.iter().zip(replies) {
                loop {
                    let message = ws.next().await.unwrap().unwrap();
                    if let Message::Pong(payload) = message {
                        assert_eq!(&payload[..], b"backend");
                        pong_received = true;
                    } else if request.contains("thread/start") {
                        // The selection gains the pane's environment and keeps the rest.
                        let forwarded: Value =
                            serde_json::from_str(message.to_text().unwrap()).unwrap();
                        let config = &forwarded["params"]["config"];
                        assert_eq!(
                            config["shell_environment_policy.set.YINZ_PANE_REF"],
                            "aaaaaaaaaaaa"
                        );
                        assert!(config["shell_environment_policy.set.YINZ_SOCKET_DIR"].is_string());
                        assert_eq!(forwarded["params"]["cwd"], "/same-directory");
                        assert_eq!(forwarded["extra"], "preserved");
                        break;
                    } else {
                        assert_eq!(message, Message::text(*request));
                        break;
                    }
                }
                ws.send(Message::text(reply)).await.unwrap();
            }
            if !pong_received {
                assert_eq!(
                    ws.next().await.unwrap().unwrap(),
                    Message::Pong(b"backend".to_vec().into())
                );
            }
            if !close_from_ui {
                ws.close(None).await.unwrap();
                if !wait_for_ack {
                    return;
                }
            }
            assert!(matches!(
                ws.next().await.unwrap().unwrap(),
                Message::Close(_)
            ));
        };
        let client = async {
            let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost", ui_stream)
                .await
                .unwrap();
            ws.send(Message::Ping(b"ui".to_vec().into())).await.unwrap();
            assert_eq!(
                ws.next().await.unwrap().unwrap(),
                Message::Pong(b"ui".to_vec().into())
            );
            for (request, reply) in requests.iter().zip(replies) {
                ws.send(Message::text(*request)).await.unwrap();
                assert_eq!(ws.next().await.unwrap().unwrap(), Message::text(reply));
            }
            assert!(binding_for(root.path(), "thread-a").unwrap().is_some());
            assert!(binding_for(root.path(), "worker").unwrap().is_none());
            if close_from_ui {
                ws.close(None).await.unwrap();
                if !wait_for_ack {
                    return;
                }
            }
            assert!(matches!(
                ws.next().await.unwrap().unwrap(),
                Message::Close(_)
            ));
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(serve, backend, client)
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn bridge_reports_a_rejected_backend_handshake() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let (ui_stream, relay_stream) = tokio::net::UnixStream::pair().unwrap();
    let (backend_stream, mut server_stream) = tokio::io::duplex(4096);
    let serve = async {
        let ui = tokio_tungstenite::accept_async(relay_stream).await.unwrap();
        let error = relay(
            ui,
            backend_stream,
            &registration,
            Path::new("/usr/bin/true"),
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("establish Codex daemon WebSocket connection"));
        assert!(format!("{error:#}").contains("403"));
    };
    let backend = async {
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(server_stream.read_u8().await.unwrap());
        }
        server_stream
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    };
    let client = async {
        let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost", ui_stream)
            .await
            .unwrap();
        let _ = ws.next().await;
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(serve, backend, client)
    })
    .await
    .unwrap();
}

fn reporting_hooks(cwd: &str, script: &Path, trust: &str) -> Value {
    let command = format!("'{}'", script.display().to_string().replace('\'', r"'\''"));
    json!({"data": [{"cwd": cwd, "errors": [], "hooks":
        yinz_types::agent::CODEX_HOOK_EVENTS.iter().map(|event| json!({
            "eventName": format!("{}{}", event[..1].to_ascii_lowercase(), &event[1..]),
            "handlerType": "command", "command": command, "enabled": true,
            "trustStatus": trust, "async": false, "matcher": null
        })).collect::<Vec<_>>()
    }]})
}

#[test]
fn capability_requires_all_the_installed_reporting_handlers() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("yinz-hook.sh");
    yinz_test_support::write_warm_script(&script, "exit 0");
    for trust in ["trusted", "managed"] {
        verify_handlers(&reporting_hooks("/work", &script, trust), "/work", &script).unwrap();
    }
    let good = reporting_hooks("/work", &script, "trusted");
    for (field, value) in [
        ("enabled", json!(false)),
        ("trustStatus", json!("untrusted")),
        ("trustStatus", json!("modified")),
        ("async", json!(true)),
        ("async", json!("false")),
        ("matcher", json!("*")),
        ("command", json!("'/unrelated/yinz-hook.sh'")),
        ("handlerType", json!("prompt")),
    ] {
        // Every required event is independently checked; unrelated hooks cannot substitute.
        for index in 0..yinz_types::agent::CODEX_HOOK_EVENTS.len() {
            let mut bad = good.clone();
            bad["data"][0]["hooks"][index][field] = value.clone();
            assert!(
                verify_handlers(&bad, "/work", &script).is_err(),
                "{field}, {index}"
            );
        }
    }
    for bad in [
        json!({}),
        json!({"data": []}),
        json!({"data":[{"cwd":"/work","errors":[],"hooks":[]}]}),
        json!({"data":[{"cwd":"/work","errors":[{"message":"bad config"}],"hooks":good["data"][0]["hooks"]}]}),
    ] {
        assert!(verify_handlers(&bad, "/work", &script).is_err());
    }
    assert!(verify_handlers(&good, "/other", &script).is_err());
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(verify_handlers(&good, "/work", &script).is_err());
    std::fs::remove_file(&script).unwrap();
    assert!(verify_handlers(&good, "/work", &script).is_err());
    std::fs::create_dir(&script).unwrap();
    assert!(verify_handlers(&good, "/work", &script).is_err());
}

#[tokio::test]
async fn capability_connection_initializes_and_checks_the_selected_thread_and_directory() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("yinz-hook.sh");
    yinz_test_support::write_warm_script(&script, "exit 0");
    for failure in [
        "none",
        "disabled",
        "missing",
        "malformed",
        "unsupported",
        "hooks_error",
        "initialize_error",
        "initialize_malformed",
        "paginated",
    ] {
        let (client, server) = tokio::io::duplex(16384);
        let check = async {
            let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost", client)
                .await
                .unwrap();
            let result = check_capability(&mut ws, "selected-thread", "/work", &script).await;
            assert_eq!(
                result.is_ok(),
                matches!(failure, "none" | "paginated"),
                "{failure}: {result:?}"
            );
        };
        let daemon = async {
            let mut ws = tokio_tungstenite::accept_async(server).await.unwrap();
            let request: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "initialize");
            assert_eq!(request["params"]["capabilities"]["experimentalApi"], true);
            if failure == "initialize_error" {
                ws.send(Message::text(
                    json!({"id":0,"error":{"message":"unsupported"}}).to_string(),
                ))
                .await
                .unwrap();
                return;
            }
            if failure == "initialize_malformed" {
                ws.send(Message::text(r#"{"id":0,"result":false}"#))
                    .await
                    .unwrap();
                return;
            }
            ws.send(Message::text(r#"{"id":0,"result":{}}"#))
                .await
                .unwrap();
            assert_eq!(
                ws.next().await.unwrap().unwrap(),
                Message::text(r#"{"method":"initialized"}"#)
            );
            let request: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "experimentalFeature/list");
            assert_eq!(request["params"]["threadId"], "selected-thread");
            let mut response_id = 1;
            if failure == "paginated" {
                ws.send(Message::text(
                    r#"{"id":1,"result":{"data":[],"nextCursor":"next"}}"#,
                ))
                .await
                .unwrap();
                let request: Value =
                    serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                assert_eq!(request["method"], "experimentalFeature/list");
                assert_eq!(request["params"]["threadId"], "selected-thread");
                assert_eq!(request["params"]["cursor"], "next");
                response_id = 2;
            }
            let features = match failure {
                "disabled" => json!({"data":[{"name":"hooks","enabled":false}]}),
                "missing" => json!({"data":[]}),
                "malformed" => json!({"data":{}}),
                _ => json!({"data":[{"name":"hooks","enabled":true}]}),
            };
            let response = if failure == "unsupported" {
                json!({"id":1,"error":{"message":"unknown method"}})
            } else {
                json!({"id":response_id,"result":features})
            };
            // Notifications and server request IDs must not be mistaken for replies.
            ws.send(Message::text(
                r#"{"id":1,"method":"server/request","params":{}}"#,
            ))
            .await
            .unwrap();
            ws.send(Message::text(response.to_string())).await.unwrap();
            if !matches!(failure, "none" | "hooks_error" | "paginated") {
                return;
            }
            let request: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "hooks/list");
            assert_eq!(request["params"]["cwds"], json!(["/work"]));
            let response = if failure == "hooks_error" {
                json!({"id":2,"error":{"message":"cannot list hooks"}})
            } else {
                json!({"id":response_id + 1,"result":reporting_hooks("/work", &script, "trusted")})
            };
            ws.send(Message::text(response.to_string())).await.unwrap();
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(check, daemon);
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn announcements_finish_before_later_selections_turns_and_the_selection_reply() {
    let root = tempfile::tempdir().unwrap();
    let entered = root.path().join("entered");
    let release = root.path().join("release");
    assert!(Command::new("mkfifo")
        .args([&entered, &release])
        .status()
        .unwrap()
        .success());
    // Read/write mode lets the test release the reporter without blocking this runtime thread.
    let mut unblock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&release)
        .unwrap();
    let reporter = root.path().join("reporter");
    yinz_test_support::write_warm_script(
        &reporter,
        &format!(
            "cat > '{}'\nprintf x > '{}'\nread released < '{}'\n",
            root.path().join("report.json").display(),
            entered.display(),
            release.display()
        ),
    );
    let mut registration = registration(root.path(), "aaaaaaaaaaaa");
    registration.reporter = Some(reporter);
    let (ui_stream, relay_stream) = tokio::net::UnixStream::pair().unwrap();
    let (backend, server) = tokio::io::duplex(16384);
    let serve = async {
        let ui = tokio_tungstenite::accept_async(relay_stream).await.unwrap();
        relay(ui, backend, &registration, Path::new("/usr/bin/true"))
            .await
            .unwrap();
    };
    let scenario = async {
        let (client, server) = tokio::join!(
            tokio_tungstenite::client_async("ws://localhost", ui_stream),
            tokio_tungstenite::accept_async(server)
        );
        let (mut ui, _) = client.unwrap();
        let mut daemon = server.unwrap();
        let selection = Message::text(r#"{"id":1,"method":"thread/start"}"#);
        ui.send(selection.clone()).await.unwrap();
        assert_eq!(daemon.next().await.unwrap().unwrap(), selection);
        let reply = Message::text(r#"{"id":1,"result":{"thread":{"id":"first"}}}"#);
        daemon.send(reply.clone()).await.unwrap();
        assert_eq!(tokio::fs::read(&entered).await.unwrap(), b"x");
        assert!(binding_for(root.path(), "first").unwrap().is_some());
        for request in [
            r#"{"id":2,"method":"thread/resume","params":{"threadId":"second"}}"#,
            r#"{"id":3,"method":"turn/start","params":{"threadId":"second"}}"#,
        ] {
            ui.send(Message::text(request)).await.unwrap();
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), daemon.next())
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), ui.next())
                .await
                .is_err()
        );
        assert!(binding_for(root.path(), "first").unwrap().is_some());
        unblock.write_all(b"done\n").unwrap();
        assert_eq!(ui.next().await.unwrap().unwrap(), reply);
        for id in [2, 3] {
            let request: Value =
                serde_json::from_str(daemon.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(request["id"], id);
        }
        assert!(binding_for(root.path(), "second").unwrap().is_some());
        let hook: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("report.json")).unwrap())
                .unwrap();
        assert_eq!(hook["_yinz_codex_hooks_ready"], false);
        ui.close(None).await.unwrap();
        assert!(matches!(
            daemon.next().await.unwrap().unwrap(),
            Message::Close(_)
        ));
    };
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        tokio::join!(serve, scenario);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn capability_timeout_still_announces_an_unverified_selection() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("codex");
    yinz_test_support::write_warm_script(&program, "cat >/dev/null");
    let reporter = root.path().join("reporter");
    let output = root.path().join("report.json");
    yinz_test_support::write_warm_script(&reporter, &format!("cat > '{}'\n", output.display()));
    let mut registration = registration(root.path(), "aaaaaaaaaaaa");
    registration.reporter = Some(reporter);
    let started = std::time::Instant::now();
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        registration.announce(
            json!({
                "session_id":"first", "cwd":"/work", "_yinz_codex_hooks_ready":true
            }),
            &program,
        ),
    )
    .await
    .unwrap();
    assert!(started.elapsed() >= std::time::Duration::from_secs(10));
    let hook: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(hook["_yinz_codex_hooks_ready"], false);
}

#[test]
fn only_an_idle_selection_can_bootstrap_codex_readiness() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = Conversations::default();
    for status in [
        Value::Null,
        json!({"type":"active"}),
        json!({"type":"notLoaded"}),
        json!({"type":"idle"}),
    ] {
        conversations
            .request(&json!({"id":1,"method":"thread/start"}), &registration)
            .unwrap();
        let hook = conversations.response(&json!({"id":1,"result":{"thread":{"id":"selected","cwd":"/work","status":status}}}), &registration).unwrap().pop().unwrap();
        assert_eq!(hook["_yinz_codex_hooks_ready"], status["type"] == "idle");
    }
}

#[tokio::test]
async fn worktree_trust_uses_only_base_user_decisions_and_guards_concurrent_writes() {
    let root = Path::new("/work/tree.with \"quotes\"");
    let primary = Path::new("/work/primary");
    for scenario in [
        "trusted",
        "approved",
        "approval_conflict",
        "untrusted",
        "unknown",
        "missing_parent",
        "existing_trusted",
        "existing_untrusted",
        "conflict",
        "conflict_untrusted",
        "conflict_parent_untrusted",
        "conflict_twice",
        "write_error",
        "bad_write",
        "bad_layers",
        "bad_config",
        "bad_projects",
        "bad_entry",
        "missing_version",
        "missing_file",
        "profile_only",
        "duplicate_user",
        "disabled",
        "bad_initialize",
        "existing_null",
    ] {
        let (client, server) = tokio::io::duplex(16384);
        let check = async {
            let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost", client)
                .await
                .unwrap();
            let result = inherit_worktree_trust(&mut ws, root, primary, || {
                Ok(matches!(scenario, "approved" | "approval_conflict"))
            })
            .await;
            assert_eq!(
                result.is_ok(),
                matches!(
                    scenario,
                    "trusted"
                        | "approved"
                        | "untrusted"
                        | "unknown"
                        | "missing_parent"
                        | "existing_trusted"
                        | "existing_untrusted"
                        | "conflict"
                        | "conflict_untrusted"
                        | "conflict_parent_untrusted"
                ),
                "{scenario}: {result:?}"
            );
            if let Ok(trusted) = result {
                assert_eq!(
                    trusted,
                    matches!(
                        scenario,
                        "trusted" | "approved" | "existing_trusted" | "conflict"
                    )
                );
            }
        };
        let daemon = async {
            let mut ws = tokio_tungstenite::accept_async(server).await.unwrap();
            let mut config = json!({"projects": {
                primary.to_str().unwrap(): {"trust_level": "trusted"},
                root.to_str().unwrap(): {"label": "preserve"},
                "/unrelated": {"trust_level": "untrusted"}
            }, "sandbox_mode": "read-only", "approval_policy": "untrusted"});
            match scenario {
                "untrusted" => {
                    config["projects"][primary.to_str().unwrap()]["trust_level"] =
                        json!("untrusted")
                },
                "unknown" => {
                    config.as_object_mut().unwrap().remove("projects");
                },
                "missing_parent" => {
                    config["projects"]
                        .as_object_mut()
                        .unwrap()
                        .remove(primary.to_str().unwrap());
                },
                "existing_trusted" => {
                    config["projects"][root.to_str().unwrap()]["trust_level"] = json!("trusted")
                },
                "existing_untrusted" | "approved" | "approval_conflict" => {
                    config["projects"][root.to_str().unwrap()]["trust_level"] = json!("untrusted")
                },
                "existing_null" => {
                    config["projects"][root.to_str().unwrap()]["trust_level"] = Value::Null
                },
                "bad_projects" => config["projects"] = json!([]),
                "bad_entry" => config["projects"][root.to_str().unwrap()] = json!(false),
                "bad_config" => config = Value::Null,
                _ => {},
            }
            let original = config.clone();
            let mut reads = 0;
            let mut writes = 0;
            while let Some(Ok(message)) = ws.next().await {
                let request: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "initialize" => {
                        if scenario == "bad_initialize" {
                            json!(false)
                        } else {
                            json!({})
                        }
                    },
                    "initialized" => continue,
                    "config/read" => {
                        assert_eq!(request["params"], json!({"includeLayers": true}));
                        reads += 1;
                        let mut user = json!({"name":{"type":"user","file":"/home/test/.codex/config.toml"},
                            "version": format!("version-{reads}"), "config":config});
                        match scenario {
                            "missing_version" => user["version"] = Value::Null,
                            "missing_file" => user["name"]["file"] = Value::Null,
                            "profile_only" => user["name"]["profile"] = json!("dev"),
                            "disabled" => user["disabledReason"] = json!("disabled"),
                            _ => {},
                        }
                        let mut layers = vec![
                            json!({"name":{"type":"sessionFlags"},"config":{"projects":{primary.to_str().unwrap():{"trust_level":"trusted"}}}}),
                            json!({"name":{"type":"user","profile":"dev"},"config":{"projects":{primary.to_str().unwrap():{"trust_level":"trusted"}}}}),
                            user.clone(),
                        ];
                        if scenario == "duplicate_user" {
                            layers.push(user);
                        }
                        if scenario == "bad_layers" {
                            json!({})
                        } else {
                            json!({"layers":layers})
                        }
                    },
                    "config/value/write" => {
                        writes += 1;
                        assert_eq!(
                            request["params"],
                            json!({
                                "keyPath":"projects", "value":{root.to_str().unwrap():{"trust_level":"trusted"}},
                                "mergeStrategy":"upsert", "filePath":"/home/test/.codex/config.toml",
                                "expectedVersion":format!("version-{reads}")
                            }),
                            "{scenario}"
                        );
                        if (scenario.starts_with("conflict") || scenario == "approval_conflict")
                            && (writes == 1 || scenario == "conflict_twice")
                        {
                            config["projects"]["/concurrent"] = json!({"trust_level":"untrusted"});
                            if scenario == "conflict_untrusted" {
                                config["projects"][root.to_str().unwrap()]["trust_level"] =
                                    json!("untrusted");
                            }
                            if scenario == "conflict_parent_untrusted" {
                                config["projects"][primary.to_str().unwrap()]["trust_level"] =
                                    json!("untrusted");
                            }
                            ws.send(Message::text(json!({"id":request["id"],"error":{
                                "code":-32600,"data":{"config_write_error_code":"configVersionConflict"}
                            }}).to_string())).await.unwrap();
                            continue;
                        }
                        if scenario == "write_error" {
                            ws.send(Message::text(json!({"id":request["id"],"error":{"code":-32600,"message":"denied"}}).to_string())).await.unwrap();
                            continue;
                        }
                        if scenario == "bad_write" {
                            json!({})
                        } else {
                            config["projects"][root.to_str().unwrap()]["trust_level"] =
                                json!("trusted");
                            json!({"status":"ok", "version":"new", "filePath":"/home/test/.codex/config.toml"})
                        }
                    },
                    method => panic!("unexpected method: {method}"),
                };
                ws.send(Message::text(
                    json!({"id":request["id"],"result":result}).to_string(),
                ))
                .await
                .unwrap();
            }
            let expected_writes = match scenario {
                "conflict" | "conflict_twice" => 2,
                "trusted"
                | "approved"
                | "approval_conflict"
                | "conflict_untrusted"
                | "conflict_parent_untrusted"
                | "write_error"
                | "bad_write" => 1,
                _ => 0,
            };
            assert_eq!(writes, expected_writes, "{scenario}");
            assert_eq!(
                reads,
                if scenario == "bad_initialize" {
                    0
                } else if scenario.starts_with("conflict") {
                    2
                } else {
                    1
                },
                "{scenario}"
            );
            if scenario.starts_with("conflict") {
                assert_eq!(
                    config["projects"]["/concurrent"],
                    json!({"trust_level":"untrusted"})
                );
            }
            if matches!(scenario, "trusted" | "approved" | "conflict") {
                assert_eq!(
                    config["projects"][root.to_str().unwrap()]["label"],
                    "preserve"
                );
                assert_eq!(
                    config["projects"]["/unrelated"],
                    original["projects"]["/unrelated"]
                );
                assert_eq!(config["approval_policy"], original["approval_policy"]);
                assert_eq!(config["sandbox_mode"], original["sandbox_mode"]);
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(check, daemon);
        })
        .await
        .unwrap();
    }
}

#[test]
fn failures_belong_only_to_the_selected_current_turn() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = Conversations::default();
    conversations
        .request(&json!({"id":1,"method":"thread/resume"}), &registration)
        .unwrap();
    conversations.response(&json!({"id":1,"result":{"thread":{
        "id":"selected", "turns":[{"id":"old","status":"completed"},{"id":"active","status":"inProgress"}]
    }}}), &registration).unwrap();
    conversations
        .request(
            &json!({"id":2,"method":"thread/start","params":{"ephemeral":true}}),
            &registration,
        )
        .unwrap();
    assert!(conversations
        .response(
            &json!({"id":2,"result":{"thread":{"id":"hidden"}}}),
            &registration
        )
        .unwrap()
        .is_empty());
    conversations
        .request(
            &json!({"id":3,"method":"turn/start","params":{"threadId":"hidden"}}),
            &registration,
        )
        .unwrap();
    for (thread, turn) in [
        ("hidden", "active"),
        ("unrelated", "active"),
        ("selected", "old"),
    ] {
        assert!(conversations.response(&json!({"method":"turn/completed","params":{
            "threadId":thread,"turn":{"id":turn,"status":"failed","error":{"message":"ignore"}}
        }}), &registration).unwrap().is_empty());
    }
    let failure = json!({"method":"turn/completed","params":{
        "threadId":"selected","turn":{"id":"active","status":"failed","error":{"message":"API refused"}}
    }});
    let hook = conversations
        .response(&failure, &registration)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        hook,
        json!({"hook_event_name":"StopFailure", "session_id":"selected", "turn_id":"active", "last_assistant_message":"API refused"})
    );
    assert!(conversations
        .response(&failure, &registration)
        .unwrap()
        .is_empty());
    conversations
        .response(
            &json!({"method":"turn/started","params":{"threadId":"selected","turn":{"id":"new"}}}),
            &registration,
        )
        .unwrap();
    conversations
        .request(
            &json!({"id":4,"method":"turn/start","params":{"threadId":"other"}}),
            &registration,
        )
        .unwrap();
    assert!(conversations.response(&json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"new","status":"failed"}}}), &registration).unwrap().is_empty());
    assert!(conversations.response(&json!({"method":"turn/completed","params":{"threadId":"other","turn":{"id":"new","status":"failed"}}}), &registration).unwrap().is_empty());
}

#[test]
fn proposed_plans_do_not_report_waiting_on_completion() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = Conversations::default();
    conversations
        .request(
            &json!({"id":1,"method":"turn/start","params":{"threadId":"selected"}}),
            &registration,
        )
        .unwrap();
    for event in [
        json!({"method":"turn/started","params":{"threadId":"selected","turn":{"id":"plan"}}}),
        json!({"method":"item/completed","params":{"threadId":"selected","turnId":"plan","item":{"type":"plan"}}}),
        json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"plan","status":"completed"}}}),
        json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"plan","status":"completed"}}}),
        // A delayed failure cannot change the completed turn either.
        json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"plan","status":"failed"}}}),
    ] {
        assert!(conversations
            .response(&event, &registration)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn resumed_plans_do_not_report_waiting_on_completion() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    for active_plan in [false, true] {
        let mut conversations = Conversations::default();
        conversations
            .request(&json!({"id":1,"method":"thread/resume"}), &registration)
            .unwrap();
        conversations
            .response(
                &json!({"id":1,"result":{"thread":{"id":"selected","turns":[
                    {"id":"old","status":"completed","items":[{"type":"plan"}]},
                    {"id":"active","status":"inProgress","items":[{"type":if active_plan { "plan" } else { "agentMessage" }}]}
                ]}}}),
                &registration,
            )
            .unwrap();
        let hooks = conversations
            .response(
                &json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"active","status":"completed"}}}),
                &registration,
            )
            .unwrap();
        assert!(hooks.is_empty());
    }
}

#[tokio::test]
async fn bridge_reports_only_failures_locally_and_remotely_without_changing_traffic() {
    for remote in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let reports = root.path().join("reports");
        let reporter = root.path().join("reporter");
        yinz_test_support::write_warm_script(
            &reporter,
            &format!(
                "printf '%s\\n' \"$*\" >> '{}'\ncat >> '{}'\nprintf '\\n' >> '{}'\n",
                reports.display(),
                reports.display(),
                reports.display()
            ),
        );
        let mut registration = registration(root.path(), "aaaaaaaaaaaa");
        if remote {
            registration
                .env
                .insert("YINZ_REMOTE_PANE_TOKEN".into(), "remote-token".into());
        }
        registration.reporter = Some(reporter);
        let (ui_stream, relay_stream) = tokio::net::UnixStream::pair().unwrap();
        let (backend_stream, server_stream) = tokio::io::duplex(16384);
        let serve = async {
            let ui = tokio_tungstenite::accept_async(relay_stream).await.unwrap();
            let result = relay(
                ui,
                backend_stream,
                &registration,
                Path::new("/usr/bin/true"),
            )
            .await;
            assert!(
                result.is_err(),
                "the mock backend disconnects without a Close frame"
            );
        };
        let scenario = async {
            let (client, server) = tokio::join!(
                tokio_tungstenite::client_async("ws://localhost", ui_stream),
                tokio_tungstenite::accept_async(server_stream)
            );
            let (mut ui, _) = client.unwrap();
            let mut backend = server.unwrap();
            let request = Message::text(
                r#"{ "id":1,"method":"turn/start","params":{"threadId":"selected"} }"#,
            );
            ui.send(request.clone()).await.unwrap();
            assert_eq!(backend.next().await.unwrap().unwrap(), request);
            let events = [
                json!({"method":"turn/started","params":{"threadId":"selected","turn":{"id":"old"}}}),
                json!({"method":"error","params":{"threadId":"selected","turnId":"old","willRetry":true,"error":{"message":"retrying"}}}),
                json!({"method":"error","params":{"threadId":"selected","turnId":"old","willRetry":false,"error":{"message":"still not final"}}}),
                json!({"method":"turn/started","params":{"threadId":"worker","turn":{"id":"worker-turn"}}}),
                json!({"method":"turn/completed","params":{"threadId":"worker","turn":{"id":"worker-turn","status":"failed"}}}),
                json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"old","status":"completed"}}}),
                json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"old","status":"failed"}}}),
                json!({"method":"turn/started","params":{"threadId":"selected","turn":{"id":"current"}}}),
                json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"old","status":"failed"}}}),
                json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"current","status":"failed","error":{"message":"HTTP 400: unsupported model"}}}}),
            ];
            for (index, event) in events.iter().enumerate() {
                let message = Message::text(format!(" {event} "));
                backend.send(message.clone()).await.unwrap();
                assert_eq!(ui.next().await.unwrap().unwrap(), message);
                if index + 1 < events.len() {
                    assert!(
                        !reports.exists(),
                        "transient, unrelated or stale event reported: {event}"
                    );
                }
            }
            let failure = Message::text(events.last().unwrap().to_string());
            backend.send(failure.clone()).await.unwrap();
            assert_eq!(ui.next().await.unwrap().unwrap(), failure);
            for event in [
                json!({"method":"turn/started","params":{"threadId":"selected","turn":{"id":"plan"}}}),
                json!({"method":"item/completed","params":{"threadId":"selected","turnId":"plan","item":{"type":"plan","id":"proposal","text":"Implement the change"}}}),
                json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"plan","status":"completed"}}}),
            ] {
                let message = Message::text(format!(" {event} "));
                backend.send(message.clone()).await.unwrap();
                assert_eq!(ui.next().await.unwrap().unwrap(), message);
            }
            // A new turn followed by a transport disconnect is not a final failure.
            let start = Message::text(json!({"method":"turn/started","params":{"threadId":"selected","turn":{"id":"next"}}}).to_string());
            backend.send(start.clone()).await.unwrap();
            assert_eq!(ui.next().await.unwrap().unwrap(), start);
            drop(backend);
            assert!(ui.next().await.unwrap().is_err());
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(serve, scenario);
        })
        .await
        .unwrap();
        let reports = std::fs::read_to_string(&reports).unwrap();
        let lines: Vec<_> = reports.lines().collect();
        assert_eq!(lines.len(), 2, "only the failure is reported: {reports}");
        assert_eq!(
            lines[0],
            if remote {
                "--pty-broker report codex"
            } else {
                "action agent-report --hook-json --kind codex"
            }
        );
        let hook: yinz_types::agent::HookEvent = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(hook.hook_event_name, "StopFailure");
        assert_eq!(hook.session_id, "selected");
        assert_eq!(hook.turn_id.as_deref(), Some("current"));
        assert_eq!(
            hook.last_assistant_message.as_deref(),
            Some("HTTP 400: unsupported model")
        );
    }
}

#[test]
fn bubblewrap_private_backend_uses_its_socket_and_dies_with_its_owner() {
    const TEST: &str = "tests::bubblewrap_private_backend_uses_its_socket_and_dies_with_its_owner";
    if let Ok(socket) = std::env::var("YINZ_TEST_PRIVATE_SOCKET") {
        eprintln!("PRIVATE_BACKEND_STDERR_MARKER");
        let listener = std::os::unix::net::UnixListener::bind(socket).unwrap();
        loop {
            let _ = listener.accept().unwrap();
        }
    }
    if std::env::var_os("YINZ_TEST_PRIVATE_PROXY").is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                use tokio::io::AsyncReadExt;
                let (mut child, mut stream) = daemon_connection(
                    Path::new(&std::env::var("YINZ_TEST_PRIVATE_PROGRAM").unwrap()),
                    Some(Path::new(PRIVATE_BACKEND)),
                )
                .unwrap();
                let mut args = String::new();
                stream.read_to_string(&mut args).await.unwrap();
                assert_eq!(
                    args,
                    format!("app-server\nproxy\n--sock\n{PRIVATE_BACKEND}\n")
                );
                assert!(child.wait().await.unwrap().success());
            });
        return;
    }
    if std::env::var_os("YINZ_TEST_PRIVATE_OWNER").is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env("YINZ_TEST_PRIVATE_OWNER", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_BACKEND_STDERR_MARKER"));
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("codex");
    yinz_test_support::write_warm_script(
        &program,
        &format!(
            r#"
if [ "$2" = proxy ]; then
    printf '%s\n' "$@"
    exit 0
fi
[ "$1" = app-server ] && [ "$2" = --listen ] || exit 99
export YINZ_TEST_PRIVATE_SOCKET="${{3#unix://}}"
exec '{}' --exact '{}' --nocapture
"#,
            std::env::current_exe().unwrap().display(),
            TEST
        ),
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let socket = root.path().join("private.sock");
            let child = private_backend(&program, &socket, &[]).await.unwrap();
            let pid = child.id().unwrap();
            drop(child);
            // SIGKILL on drop is observed through waitpid, without a scheduling-dependent sleep.
            let status = tokio::task::spawn_blocking(move || {
                nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid as i32), None)
            })
            .await
            .unwrap();
            assert!(
                matches!(
                    status,
                    Ok(nix::sys::wait::WaitStatus::Signaled(
                        _,
                        nix::sys::signal::Signal::SIGKILL,
                        _
                    )) | Err(nix::errno::Errno::ECHILD)
                ),
                "{status:?}"
            );
        });
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env("YINZ_TEST_PRIVATE_PROXY", "1")
        .env("YINZ_TEST_PRIVATE_PROGRAM", &program)
        .env("YINZ_BUBBLEWRAP", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn project_trust_override_survives_bridge_and_conversation_reset() {
    let work = "/tmp/Project's workspace";
    let env = HashMap::from([("YINZ_PANE_REF".into(), "project-pane".into())]);
    let config = json!({
        "projects": {work: {"trust_level": "trusted"}},
        "sandbox_workspace_write.network_access": true,
    });
    assert!(bridge_options(&[
        "-c".into(),
        format!(
            "projects={{{}={{trust_level=\"trusted\"}}}}",
            serde_json::to_string(work).unwrap()
        ),
        "-c".into(),
        "sandbox_workspace_write.network_access=true".into(),
    ])
    .is_some());
    for method in ["thread/start", "thread/resume", "thread/fork"] {
        let mut request = json!({"method": method, "params": {"cwd": work, "config": config}});
        assert!(inject_pane_env(&mut request, &env));
        assert_eq!(request["params"]["config"]["projects"], config["projects"]);
        assert_eq!(
            request["params"]["config"]["sandbox_workspace_write.network_access"],
            true
        );
        assert_eq!(
            request["params"]["config"]["shell_environment_policy.set.YINZ_PANE_REF"],
            "project-pane"
        );
    }
}

#[tokio::test]
async fn bridge_relays_daemon_frames_larger_than_the_default_limit() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let (ui_stream, relay_stream) = tokio::net::UnixStream::pair().unwrap();
    let (backend_stream, server_stream) = tokio::io::duplex(1 << 16);
    // The daemon's whole `plugin/list` catalog arrives as one frame beyond 16 MiB.
    let default_limit = WebSocketConfig::default().max_frame_size.unwrap();
    let padding = "x".repeat(default_limit + (1 << 16));
    let request = r#"{"id":7,"method":"plugin/list","params":{}}"#;
    let reply = format!(r#"{{"id":7,"result":{{"plugins":["{padding}"]}}}}"#);
    let serve = async {
        let ui =
            tokio_tungstenite::accept_async_with_config(relay_stream, Some(websocket_config()))
                .await
                .unwrap();
        relay(
            ui,
            backend_stream,
            &registration,
            Path::new("/usr/bin/true"),
        )
        .await
        .unwrap();
    };
    let backend = async {
        let mut ws = tokio_tungstenite::accept_async(server_stream)
            .await
            .unwrap();
        assert_eq!(ws.next().await.unwrap().unwrap(), Message::text(request));
        ws.send(Message::text(reply.as_str())).await.unwrap();
        assert!(matches!(
            ws.next().await.unwrap().unwrap(),
            Message::Close(_)
        ));
    };
    let client = async {
        let (mut ws, _) = tokio_tungstenite::client_async_with_config(
            "ws://localhost",
            ui_stream,
            Some(websocket_config()),
        )
        .await
        .unwrap();
        ws.send(Message::text(request)).await.unwrap();
        let Message::Text(received) = ws.next().await.unwrap().unwrap() else {
            panic!("expected the daemon's text reply");
        };
        assert!(received.len() > default_limit);
        assert!(received.as_str() == reply);
        ws.close(None).await.unwrap();
        assert!(matches!(
            ws.next().await.unwrap().unwrap(),
            Message::Close(_)
        ));
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(serve, backend, client)
    })
    .await
    .unwrap();
}

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
        assert!(bridge_eligible(&["--model".into(), value.into()]));
        for args in [
            vec!["-c".into(), value.into()],
            vec!["--config".into(), value.into()],
            vec![format!("-c{value}")],
            vec![format!("-c={value}")],
            vec![format!("--config={value}")],
        ] {
            assert_eq!(bridge_eligible(&args), expected, "{args:?}");
            let mut literal = vec!["--".into()];
            literal.extend(args);
            assert!(bridge_eligible(&literal), "{literal:?}");
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
        assert_eq!(bridge_eligible(&args), *expected, "{args:?}");
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
            !bridge_eligible(&["-c".into(), "model=example".into(), utility.into()]),
            "{utility}"
        );
    }
}

#[test]
fn registry_and_a_session_named_codex_coexist() {
    const CHILD_ENV: &str = "ZELLIJ_TEST_CODEX_REGISTRY";
    const THREAD_ENV: &str = "ZELLIJ_TEST_CODEX_THREAD";
    const SOCKETS_ENV: &str = "ZELLIJ_TEST_CODEX_SOCKETS";
    const TEST: &str = "codex::tests::registry_and_a_session_named_codex_coexist";
    if std::env::var_os(CHILD_ENV).is_none() {
        for source in ["fallback", "runtime", "override"] {
            let root = tempfile::tempdir().unwrap();
            let mut launcher = Command::new(std::env::current_exe().unwrap());
            launcher
                .args(["--exact", TEST])
                .env(CHILD_ENV, source)
                .env("HOME", root.path())
                .env("TMPDIR", root.path())
                .env_remove("ZELLIJ_SOCKET_DIR")
                .env_remove("XDG_RUNTIME_DIR");
            if source != "fallback" {
                launcher.env("XDG_RUNTIME_DIR", root.path().join("runtime"));
            }
            if source == "override" {
                launcher.env("ZELLIJ_SOCKET_DIR", root.path().join("sockets"));
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
            Path::new(&binding.env["ZELLIJ_SOCKET_DIR"])
                .join(&*zellij_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
                .canonicalize()
                .unwrap(),
            sockets.canonicalize().unwrap()
        );
        if std::env::var_os("XDG_RUNTIME_DIR").is_some() {
            assert_ne!(&*zellij_runtime::consts::ZELLIJ_SOCK_DIR, &sockets);
        }
        return;
    }
    let sockets = &*zellij_runtime::consts::ZELLIJ_SOCK_DIR;
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
            .env_remove("ZELLIJ_SOCKET_DIR")
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
    Registration::new(
        root,
        HashMap::from([("ZELLIJ_PANE_REF".into(), pane.into())]),
    )
    .unwrap()
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
        binding_for(root.path(), "thread-a").unwrap().unwrap().env["ZELLIJ_PANE_REF"],
        "aaaaaaaaaaaa"
    );
    assert_eq!(
        binding_for(root.path(), "thread-b").unwrap().unwrap().env["ZELLIJ_PANE_REF"],
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

#[tokio::test]
async fn bridge_registers_before_delivering_replies_and_preserves_the_protocol() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let program = root.path().join("codex");
    zellij_test_support::write_warm_script(
        &program,
        r#"#!/bin/sh
[ "$1 $2" = 'app-server proxy' ] || exit 1
IFS= read -r request
printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread-a"},"extra":"preserved"}}'
IFS= read -r request
printf '%s\n' '{"method":"turn/started","params":{"threadId":"thread-a"}}'
cat >/dev/null
"#,
    );
    let socket = registration.dir.path().join("test.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let serve = async {
        let (stream, _) = listener.accept().await.unwrap();
        proxy(stream, &program, &registration).await.unwrap();
    };
    let client = async {
        let stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
        let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost", stream)
            .await
            .unwrap();
        ws.send(Message::text(
            r#"{"id":1,"method":"thread/start","params":{"cwd":"/same-directory"}}"#,
        ))
        .await
        .unwrap();
        let reply = ws.next().await.unwrap().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&reply).unwrap()["result"]["extra"],
            "preserved"
        );
        assert!(binding_for(root.path(), "thread-a").unwrap().is_some());
        ws.send(Message::text(
            r#"{"id":2,"method":"turn/start","params":{"threadId":"thread-a"}}"#,
        ))
        .await
        .unwrap();
        let reply = ws.next().await.unwrap().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&reply).unwrap()["method"],
            "turn/started"
        );
        ws.close(None).await.unwrap();
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(serve, client)
    })
    .await
    .unwrap();
}

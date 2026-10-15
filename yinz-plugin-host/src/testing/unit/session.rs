use super::*;

#[test]
fn bounded_post_shutdown_diagnostics_keep_the_owned_pid() {
    let root = tempfile::tempdir().unwrap();
    let child = Command::new("/usr/bin/true").spawn().unwrap();
    let mut session = OwnedSession {
        root,
        child,
        stopped: false,
    };
    session.child.wait().unwrap();
    session.shutdown().unwrap();
    assert!(session.stopped);
    assert!(session.child.try_wait().unwrap().is_some());
    fs::write(session.root().join("server.log"), "λ".repeat(OUTPUT_LIMIT)).unwrap();
    fs::write(
        session.root().join("diagnostics"),
        format!("{}\npost-shutdown evidence\n", "x".repeat(OUTPUT_LIMIT)),
    )
    .unwrap();
    let diagnostics = session.diagnostics();
    let mut output = "case output".repeat(OUTPUT_LIMIT);
    append_bounded(&mut output, &diagnostics);
    assert!(output.len() <= OUTPUT_LIMIT);
    assert!(output.contains("post-shutdown evidence"));
    assert!(output.ends_with(&format!("owned server pid: {}\n", session.child.id())));
}

#[test]
fn shutdown_reaps_the_child_even_when_ipc_connection_fails() {
    let root = tempfile::tempdir().unwrap();
    let child = Command::new("/bin/sh")
        .args(["-c", "read release"])
        .process_group(0)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut session = OwnedSession {
        root,
        child,
        stopped: false,
    };
    let error = session.shutdown().unwrap_err();
    assert!(format!("{error:#}").contains("shutdown IPC: connect to owned test session"));
    assert!(session.stopped);
    assert!(session.child.try_wait().unwrap().is_some());
    assert!(session.root().exists());
    assert!(session.diagnostics().contains("owned server pid:"));
}

#[test]
fn locations_preserve_paths_and_confine_targets() {
    let root = tempfile::tempdir().unwrap();
    let entry = root.path().join("case.test.scm");
    fs::write(&entry, "").unwrap();
    let loader = super::super::steel::Sources {
        entry: entry.clone(),
        root: root.path().canonicalize().unwrap(),
    };
    let entry_url = url::Url::from_file_path(entry.canonicalize().unwrap()).unwrap();
    let controller = Controller::new(root.path().to_owned(), vec![]);
    for name in yinz_runtime::consts::BUILTIN_PLUGIN_NAMES {
        let target = format!("yinz:{name}");
        assert_eq!(controller.location(&target, &loader).unwrap(), target);
    }
    for name in [
        "space here",
        "日本語",
        "hash#",
        "query?",
        "%23",
        "%2523",
        "reader 日本 #?%23%2523",
    ] {
        let path = root.path().join(format!("{name}.scm"));
        fs::write(&path, "").unwrap();
        let path = path.canonicalize().unwrap();
        let url = url::Url::from_file_path(&path).unwrap();
        for target in [
            url.to_string(),
            format!("./{}", url.path().rsplit('/').next().unwrap()),
        ] {
            let resolved = controller.location(&target, &loader).unwrap();
            assert_eq!(resolved, url.as_str());
            assert_eq!(
                yinz_runtime::plugin_location::parse_plugin_location(&resolved, None).unwrap(),
                yinz_types::input::layout::RunPluginLocation::File(path.clone())
            );
        }
    }
    for name in ["other.wasm", "other.txt"] {
        fs::write(root.path().join(name), "").unwrap();
    }
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("external.scm");
    fs::write(&external, "").unwrap();
    std::os::unix::fs::symlink(&external, root.path().join("escape.scm")).unwrap();
    let external = url::Url::from_file_path(external).unwrap();
    for target in [
        "yinz:missing",
        "yinz:file-finder?x",
        "yinz:file-finder#x",
        "file-finder",
        "../outside.scm",
        "./escape.scm",
        "https://example.com/plugin.scm",
        "./other.wasm",
        "./other.txt",
        "./case.test.scm?x",
        "./case.test.scm#x",
        "yinz:plugin",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([
        format!("{}?x", entry_url),
        format!("{}#x", entry_url),
        entry_url.make_relative(&external).unwrap(),
        external.to_string(),
    ]) {
        assert!(controller.location(&target, &loader).is_err(), "{target}");
    }
}

#[test]
fn raw_actions_cannot_launch_or_reconfigure() {
    let root = tempfile::tempdir().unwrap();
    let mut controller = Controller::new(root.path().to_owned(), vec![]);
    for action in [
        "LaunchPlugin \"yinz:file-finder\"",
        "StartOrReloadPlugin \"yinz:file-finder\"",
        "Reconfigure \"load_plugins {}\"",
        "NewPane { command \"/usr/bin/true\"; }",
        "NewPane cwd=\"/tmp\"",
    ] {
        let error = controller.actions(action).unwrap_err().to_string();
        assert!(
            error.contains("unsupported test action") || error.contains("cannot supply"),
            "{error}"
        );
    }
}

#[test]
fn disposable_sessions_do_not_schedule_durable_metadata_checkpoints() {
    let mut session = OwnedSession::new().unwrap();
    let entry = session.root().join("case.test.scm");
    fs::write(&entry, "").unwrap();
    let loader = super::super::steel::Sources {
        entry: entry.clone(),
        root: session.root().canonicalize().unwrap(),
    };
    let mut controller = Controller::new(session.root().to_owned(), vec![]);
    controller.call("start", json!({}), &loader).unwrap();
    session.shutdown().unwrap();
    let diagnostics = session.diagnostics();
    assert!(
        diagnostics.contains("background jobs received Exit"),
        "{diagnostics}"
    );
    assert!(
        !diagnostics.contains("aborting checkpoint task"),
        "disposable servers must not wait for durable checkpoint IO: {diagnostics}"
    );
}

#[test]
fn connected_repl_cli_evaluates_with_session_host_access() {
    let mut session = OwnedSession::new().unwrap();
    let entry = session.root().join("case.test.scm");
    fs::write(&entry, "").unwrap();
    let loader = super::super::steel::Sources {
        entry: entry.clone(),
        root: session.root().canonicalize().unwrap(),
    };
    let mut controller = Controller::new(session.root().to_owned(), vec![]);
    controller.call("start", json!({}), &loader).unwrap();
    let mut command = Command::new(binary().unwrap());
    environment(&mut command, session.root());
    let output = command
        .args([
            "--session",
            "plugin-test",
            "repl",
            "--eval",
            r#"(define id (hash-ref (yinz-command "GetPluginIds") "plugin_id"))
               (if (number? id) (+ 20 22) (error "missing plugin ID"))"#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        session.diagnostics()
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
    session.shutdown().unwrap();
}

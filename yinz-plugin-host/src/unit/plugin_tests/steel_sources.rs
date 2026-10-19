//! Repository pinning, source identity and filesystem notifications with Scheme plugins.
use super::script_harness::{load, run_plugin, Harness};
use super::*;
use std::time::Duration;
use yinz_types::internal_ids::{PluginId, TabId};
const CLIENT: ClientId = ClientId::new(1);
const PLUGIN: PluginId = PluginId::new(0);

#[test]
#[serial(plugin_tests)]
fn steel_change_host_folder_moves_the_host_root() {
    let harness = Harness::new(
        "scm",
        r#"
(define state "starting")
(define (load config)
  (yinz-command (hash "Subscribe" (list "CustomMessage" "HostFolderChanged" "FailedToChangeHostFolder")))
  (yinz-on-event "HostFolderChanged"
    (lambda (folder)
      (define parts (split-many folder "/"))
      (set! state (string-append "moved to " (last parts) ": "
        (yinz-decode-utf8 (yinz-command (hash "FsRead" (hash "path" "/host/marker" "max_bytes" json-null "offset" json-null)))))) #t))
  (yinz-on-event "FailedToChangeHostFolder" (lambda (failure) (set! state (string-append "failed: " failure)) #t))
  (yinz-then (yinz-wait-for-permissions (list "ReadFiles" "FullHdAccess"))
    (lambda (ignored)
      (set! state (if (json-null? (yinz-command (hash "FsMetadata" (hash "path" "/host/marker")))) "before: no marker" "before: marker"))
      (yinz-request-render))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "CustomMessage"))
    (yinz-command (hash "ChangeHostFolder" (cadr (hash-ref event "CustomMessage"))))) #f)
(define (render rows cols) state)
"#,
        PermissionStatus::Granted,
    );
    let elsewhere = tempdir().unwrap();
    std::fs::write(elsewhere.path().join("marker"), "found in the new host").unwrap();
    harness.load();
    harness.rendered("before: no marker");
    harness.event(Event::CustomMessage(
        "move".into(),
        elsewhere.path().display().to_string(),
    ));
    let frame = harness.rendered("moved to");
    assert_eq!(
        frame.trim_end(),
        format!(
            "moved to {}: found in the new host",
            elsewhere.path().file_name().unwrap().to_string_lossy()
        )
    );
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("failed:")));
}

#[test]
#[serial(plugin_tests)]
fn steel_filesystem_watch_events_reach_the_plugin_and_its_reloaded_replacement() {
    let source = r#"
(define label "first")
(define seen '())
(define (load config)
  (yinz-command (hash "Subscribe" (list "FileSystemCreate" "FileSystemUpdate" "FileSystemDelete")))
  (yinz-command "WatchFilesystem"))
(define (update event)
  (define type (if (hash? event) (car (hash-keys->list event)) event))
  (if (member type (list "FileSystemCreate" "FileSystemUpdate" "FileSystemDelete"))
    (begin
      (for-each (lambda (pair)
        (set! seen (append seen (list (string-append (string-downcase (substring type 10 (string-length type))) ":"
          (last (split-many (car pair) "/")) (if (json-null? (cadr pair)) "" "?")))))) (hash-ref event type)) #t)
    #f))
(define (render rows cols) (string-append label " " (string-join seen " ")))
"#;
    let harness = Harness::new("scm", source, PermissionStatus::Granted);
    harness.load();
    harness.rendered("first");
    let watched = harness.directory.path().join("watched.txt");
    std::fs::write(&watched, "one").unwrap();
    harness.rendered("create:watched.txt");
    std::fs::write(&watched, "two").unwrap();
    harness.rendered("update:watched.txt");
    std::fs::remove_file(&watched).unwrap();
    harness.rendered("delete:watched.txt");
    // The replacement owns a fresh watcher; old callbacks retain the old generation and
    // cannot deliver their events to it.
    std::fs::write(&harness.path, source.replace("\"first\"", "\"second\"")).unwrap();
    harness.reload();
    harness.rendered("second ");
    std::fs::write(harness.directory.path().join("later.txt"), "three").unwrap();
    let frame = harness.rendered("create:later.txt");
    assert!(frame.starts_with("second "), "{frame}");
    assert!(
        !frame.contains("watched.txt"),
        "fresh state after reload: {frame}"
    );
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_scan_host_folder_answers_the_requesting_generation() {
    let source = r#"
(define label "first")
(define listing "unscanned")
(define (load config)
  (yinz-command (hash "Subscribe" (list "FileSystemUpdate")))
  (yinz-command (hash "ScanHostFolder" "/host")))
(define (update event)
  (if (and (hash? event) (hash-contains? event "FileSystemUpdate"))
    (begin
      (set! listing (string-join (sort (filter (lambda (path) (and (>= (string-length path) 4) (equal? (substring path (- (string-length path) 4) (string-length path)) ".txt")))
        (map car (hash-ref event "FileSystemUpdate"))) string<?) ",")) #t)
    #f))
(define (render rows cols) (string-append label " " listing))
"#;
    let harness = Harness::new("scm", source, PermissionStatus::Granted);
    std::fs::write(harness.directory.path().join("b.txt"), "").unwrap();
    std::fs::write(harness.directory.path().join("a.txt"), "").unwrap();
    harness.load();
    let frame = harness.rendered("first /host");
    assert_eq!(frame.trim_end(), "first /host/a.txt,/host/b.txt");
    std::fs::write(harness.directory.path().join("c.txt"), "").unwrap();
    std::fs::write(&harness.path, source.replace("\"first\"", "\"second\"")).unwrap();
    harness.reload();
    let frame = harness.rendered("second /host");
    assert_eq!(
        frame.trim_end(),
        "second /host/a.txt,/host/b.txt,/host/c.txt"
    );
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_plugin_receives_its_configuration_unnormalized() {
    let harness = Harness::new(
        "scm",
        r#"
(define state "loading")
(define (load config)
  (set! state (string-join (map (lambda (key) (string-append key "=" (hash-ref config key)))
    (sort (hash-keys->list config) string<?)) " ")))
(define (render rows cols) state)
"#,
        PermissionStatus::Granted,
    );
    let mut configuration =
        PluginUserConfiguration::new(BTreeMap::from([("greeting".into(), "configured".into())]));
    // Inserted after construction, exactly as the reserved-word filter intends to allow.
    configuration.insert("payload", "carried");
    configuration.insert("name", "yinz");
    configuration.insert("Mixed_Case-key", "kept as written");
    load(
        &harness.sender,
        RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(harness.path.clone()),
            configuration,
            ..Default::default()
        }),
    );
    let frame = harness.rendered("greeting=");
    assert_eq!(
        frame.trim_end(),
        "Mixed_Case-key=kept as written greeting=configured name=yinz payload=carried"
    );
    harness.finish();
}

#[test]
fn steel_git_plugin_first_load_failure_recovers_by_id_with_original_configuration_and_pane() {
    use yinz_types::input::git_plugin::GitPlugin;
    let harness = Harness::new("scm", "(void)", PermissionStatus::Granted);
    let path = harness.directory.path().join("missing.scm");
    load(
        &harness.sender,
        RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::Git(GitPlugin {
                repo: harness.directory.path().to_string_lossy().into_owned(),
                entrypoint: "missing.scm".into(),
                revision: None,
            }),
            configuration: PluginUserConfiguration::new(BTreeMap::from([(
                "greeting".into(),
                "original configuration".into(),
            )])),
            initial_cwd: Some(harness.directory.path().to_owned()),
            ..Default::default()
        }),
    );
    harness.log.wait_until("repository preparation failed", default_timeout(), |log| {
        log.iter().any(|i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage) if *id == PLUGIN && stage.is_error())).then_some(())
    });
    std::fs::write(
        path,
        r#"(define greeting "") (define (load config) (set! greeting (hash-ref config "greeting"))) (define (render rows cols) (string-append greeting " " (number->string rows) " " (number->string cols)))"#,
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginWithId(PLUGIN))
        .unwrap();
    harness.rendered("original configuration 10 80");
    let log = harness.finish();
    let panes = log
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::AddPlugin(_, _, _, _, _, tab, id, ..) => Some((*id, *tab)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        panes,
        [(PLUGIN, Some(TabId::new(1)))],
        "recovery must retain the original pane"
    );
}

#[test]
fn steel_git_plugin_aliases_refresh_on_config_reload_for_new_launches() {
    use yinz_types::input::git_plugin::GitPlugin;
    let harness = Harness::new("scm", "(void)", PermissionStatus::Granted);
    std::fs::create_dir(harness.directory.path().join("src")).unwrap();
    std::fs::write(
        harness.directory.path().join("src/main.scm"),
        r#"(require "../shared.scm")
           (define own-location (yinz-plugin-location))
           (define sibling-location (yinz-plugin-resolve "../shared.scm"))
           (define (render rows cols) (string-append value "\n" own-location "\n" sibling-location))"#,
    )
    .unwrap();
    std::fs::write(
        harness.directory.path().join("shared.scm"),
        r#"(provide value) (define value "reconfigured repository")"#,
    )
    .unwrap();
    let repository = GitPlugin {
        repo: harness.directory.path().to_string_lossy().into_owned(),
        entrypoint: "src/main.scm".into(),
        revision: None,
    };
    let plugin = RunPlugin {
        location: RunPluginLocation::Git(repository.clone()),
        ..Default::default()
    };
    harness
        .sender
        .send(PluginInstruction::Reconfigure {
            plugin_aliases: PluginAliases::from_data(BTreeMap::from([(
                "repository".into(),
                plugin,
            )])),
            client_id: CLIENT,
            keybinds: None,
            default_mode: None,
            default_shell: None,
            layout_dir: None,
            user_plugin_dir: None,
            hooks: Default::default(),
            context_menu: Default::default(),
            was_written_to_disk: false,
        })
        .unwrap();
    load(
        &harness.sender,
        RunPluginOrAlias::Alias(yinz_types::input::layout::PluginAlias::new(
            "repository",
            &None,
            None,
        )),
    );
    let frame = harness.rendered("reconfigured repository");
    assert!(frame.contains(&repository.descriptor()));
    let mut sibling = repository;
    sibling.entrypoint = "shared.scm".into();
    assert!(frame.contains(&sibling.descriptor()));
    harness.finish();
}

#[test]
fn steel_git_plugins_pin_imports_workers_and_attached_clients_until_reload() {
    use std::{
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };
    use yinz_types::input::git_plugin::GitPlugin;
    struct GitDaemon(std::process::Child);
    impl Drop for GitDaemon {
        fn drop(&mut self) {
            // Git spawns git-daemon, which can spawn connection workers.
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(self.0.id() as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = self.0.wait();
        }
    }
    let harness = Harness::new("scm", "(void)", PermissionStatus::Granted);
    let repository = tempdir().unwrap();
    let git = |args: &[&str]| {
        let result = Command::new("git")
            .current_dir(repository.path())
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Plugin test",
                "-c",
                "user.email=plugin@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    std::fs::create_dir(repository.path().join("src")).unwrap();
    std::fs::write(
        repository.path().join("src/main.scm"),
        format!(
            "{}{}",
            include_str!("fixtures/data_helpers.scm"),
            r#"
(require "../shared.scm")
(require "../late.scm")
(define parent "idle")
(define worker "idle")
(define saved "pending")
(define own-location (yinz-plugin-location))
(define sibling-location (yinz-plugin-resolve "../late.scm"))
(yinz-register-workers (hash "probe" (lambda (name payload)
  (yinz-post-plugin "reply" (yinz-json-string (list late-value (yinz-plugin-location)
    (yinz-plugin-resolve "../late.scm")))))))
(define (load config)
  (yinz-then (yinz-wait-for-permissions (list "PluginData"))
    (lambda (ignored)
      (yinz-data-define json-null (data-definition "markers" (list (list "value" "Text"))))
      (when (= (yinz-data-count json-null (data-query "markers")) 0)
        (yinz-data-apply json-null (list (data-put "markers" "marker"
          (list (list "value" (hash "Text" value))) json-null))))
      (set! saved (data-value (yinz-data-fetch json-null (data-query "markers")) "value"))
      (yinz-command (hash "Subscribe" (list "CustomMessage"))))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "CustomMessage"))
    (define message (hash-ref event "CustomMessage"))
    (cond
      ((equal? (car message) "probe") (set! parent late-value) (yinz-post-worker "probe" "go" ""))
      ((equal? (car message) "reply")
        (define reply (yinz-json-parse (cadr message)))
        (assert-data (equal? (cadr reply) own-location))
        (assert-data (equal? (caddr reply) sibling-location))
        (set! worker (car reply)))))
  #t)
(define (render rows cols) (string-append value " " parent " " worker " " saved "\n" own-location "\n" sibling-location))
"#
        ),
    )
    .unwrap();
    let write_revision = |value: &str| {
        for name in ["shared.scm", "late.scm"] {
            std::fs::write(
                repository.path().join(name),
                format!(
                    r#"(provide {binding}) (define {binding} "{value}")"#,
                    binding = if name == "shared.scm" {
                        "value"
                    } else {
                        "late-value"
                    }
                ),
            )
            .unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-m", value]);
    };
    write_revision("one");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let daemon = Command::new("git")
        .process_group(0)
        .args([
            "daemon",
            "--export-all",
            "--listen=127.0.0.1",
            &format!("--port={port}"),
            &format!("--base-path={}", repository.path().display()),
            repository.path().to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let daemon = GitDaemon(daemon);
    let ready = Watched::new(false);
    let notify = ready.clone();
    let probe = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        *notify.lock() = true;
    });
    ready.wait_until("Git daemon", default_timeout(), |ready| ready.then_some(()));
    probe.join().unwrap();
    let source = GitPlugin {
        repo: format!("git://127.0.0.1:{port}/"),
        entrypoint: "src/main.scm".into(),
        revision: None,
    };
    let run = RunPluginOrAlias::RunPlugin(RunPlugin {
        location: RunPluginLocation::Git(source.clone()),
        ..Default::default()
    });
    load(&harness.sender, run.clone());
    wait_for_plugin_to_handle(&harness.log, PLUGIN, &["one idle idle one"]);
    let loading_error = harness
        .log
        .lock()
        .iter()
        .find_map(|instruction| match instruction {
            ScreenInstruction::UpdatePluginLoadingStage(id, stage)
                if *id == PLUGIN && stage.is_error() =>
            {
                Some(format!("{stage:?}"))
            },
            _ => None,
        });
    assert!(
        loading_error.is_none(),
        "Git plugin failed to load: {loading_error:?}"
    );
    harness.rendered("one idle idle one");
    let first = crate::plugin_authoring::git::status(harness.directory.path(), &source)
        .unwrap()
        .unwrap();
    let mut pinned = source.clone();
    pinned.revision = first
        .commit
        .clone()
        .map(yinz_types::input::git_plugin::GitRevision::Commit);
    let frame = harness.rendered("one idle idle one");
    assert!(frame.contains(&pinned.descriptor()));
    pinned.entrypoint = "late.scm".into();
    assert!(frame.contains(&pinned.descriptor()));
    write_revision("two");
    let next = crate::plugin_authoring::git::install(
        harness.directory.path(),
        &source,
        std::slice::from_ref(&source),
        true,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert_ne!(first.commit, next.commit);
    // Offline workers and new clients must still read the original module graph and worktree.
    drop(daemon);
    harness.event(Event::CustomMessage("probe".into(), "".into()));
    harness.rendered("one one one");
    let client = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(client))
        .unwrap();
    harness.log.wait_until(
        "attached client on original revision",
        default_timeout(),
        |log| {
            rendered_assets(log)
                .iter()
                .any(|(id, text)| *id == client && text.contains("one idle idle one"))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run))
        .unwrap();
    // The probe targets client 1. Its async load must finish subscribing before the
    // event is sent; client 2's render or a frame with pending data cannot establish that.
    harness.log.wait_until(
        "original client ready on reloaded revision",
        default_timeout(),
        |log| {
            rendered_assets(log)
                .iter()
                .any(|(id, text)| *id == CLIENT && text.contains("two idle idle one"))
                .then_some(())
        },
    );
    harness.event(Event::CustomMessage("probe".into(), "".into()));
    harness.rendered("two two two one");
    // The source, not the selected path, remains the permission and private-data identity.
    let location = RunPluginLocation::Git(source);
    let identity = url::Url::from(&location).to_string();
    assert!(!identity.contains(first.root.to_str().unwrap()));
    let permissions = yinz_config::permission::PermissionCache::from_path_or_default(Some(
        harness.directory.path().join("permissions.kdl"),
    ));
    assert!(permissions
        .get_permissions(location.to_string())
        .unwrap()
        .contains(&PermissionType::PluginData));
    assert!(permissions
        .get_permissions(first.entrypoint.to_string_lossy().into_owned())
        .is_none());
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_headless_hooks_deliver_the_first_payload_after_async_permissions_and_reload() {
    use yinz_types::input::hooks::HookPoint;
    let harness = Harness::new(
        "scm",
        r#"
(define ready (yinz-deferred))
(define (say event)
  (define payload (hash-ref event "payload"))
  (unless (and (equal? (hash-ref event "point") "pane_exit")
               (= (hash-ref payload "exit_code") 0) (hash-ref payload "held"))
    (error "hook wire payload"))
  (yinz-command (hash "RenameTabWithId"
    (list 1 (string-append "steel hook:" (number->string (hash-ref payload "pane_id")))))))
(define (load config)
  (yinz-on-event "Hook" (lambda (event) (yinz-then ready (lambda (ignored) (say event)))))
  (yinz-resolve! ready (yinz-wait-for-permissions (list "ChangeApplicationState"))))
"#,
        PermissionStatus::Granted,
    );
    super::script_plugins::bind_hook(
        &harness.sender,
        HookPoint::PaneExit,
        run_plugin(&harness.path),
    );
    let fire = |id| {
        harness
            .sender
            .send(PluginInstruction::RunHook {
                point: HookPoint::PaneExit,
                pane_id: Some(PaneId::Terminal(crate::TerminalId::new(id))),
                payload: super::script_plugins::pane_exit_payload(id),
                client_id: CLIENT,
            })
            .unwrap()
    };
    let observed = |name: &str| {
        harness.log.wait_until_described("the detached client's headless Scheme hook changed the tab", Duration::from_secs(20), |log| {
            log.iter().any(|instruction| matches!(instruction, ScreenInstruction::RenameTabWithId(_, bytes, _) if bytes == name.as_bytes())).then_some(())
        }, |log| format!("{log:?}"))
    };
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    fire(7);
    observed("steel hook:7");
    fire(8);
    observed("steel hook:8");
    harness.reload();
    fire(9);
    observed("steel hook:9");
}

#[test]
#[serial(plugin_tests)]
fn steel_native_compilation_failure_allows_healthy_entrypoints_and_imports_afterward() {
    for imported in [false, true] {
        let recursive = format!(
            "(define recursive {}0{})",
            "(".repeat(10_000),
            ")".repeat(10_000)
        );
        let entry = "(require \"./dep.scm\")";
        let harness = Harness::new(
            "scm",
            if imported { entry } else { &recursive },
            PermissionStatus::Granted,
        );
        let dep = harness.directory.path().join("dep.scm");
        if imported {
            std::fs::write(&dep, &recursive).unwrap();
        }
        harness.load();
        harness.log.wait_until("confined compilation failure is reported", Duration::from_secs(40), |log| {
            log.iter().any(|instruction| matches!(instruction,
                ScreenInstruction::UpdatePluginLoadingStage(id, stage) if *id == PLUGIN && stage.is_error())).then_some(())
        });
        std::fs::write(
            &dep,
            "(provide healthy) (define healthy \"healthy after compilation failure\")",
        )
        .unwrap();
        std::fs::write(
            &harness.path,
            "(require \"./dep.scm\") (define (render rows cols) healthy)",
        )
        .unwrap();
        harness.reload();
        harness.rendered("healthy after compilation failure");
    }
}

#[test]
#[cfg(not(target_os = "macos"))]
#[serial(plugin_tests)]
fn steel_exhaustion_disposes_the_guest_and_reload_recovers_in_a_live_session() {
    let harness = Harness::new(
        "scm",
        r#"
(define (load config) (yinz-command (hash "Subscribe" (list "CustomMessage" "BeforeClose"))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "CustomMessage"))
    (with-handler (lambda (failure) #f) (make-string 2147483647 #\a)))
  (when (equal? event "BeforeClose") (error "EXHAUSTED GUEST CLEANUP RAN")) #f)
(define (render rows cols) "quota guest ready")
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("quota guest ready");
    // The worker's one-GiB address-space cap rejects this allocation before physical memory grows.
    harness.event(super::script_harness::release_event());
    harness.log.wait_until("worker allocation failure is reported", Duration::from_secs(20), |log| {
        log.iter().any(|instruction| matches!(instruction,
            ScreenInstruction::UpdatePluginLoadingStage(id, stage) if *id == PLUGIN && stage.is_error())).then_some(())
    });
    let survivor = harness.directory.path().join("survivor.scm");
    std::fs::write(
        &survivor,
        "(define (render rows cols) \"other plugin survived quota\")",
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("other plugin survived quota");
    std::fs::write(
        &harness.path,
        "(define (render rows cols) \"quota replacement healthy\")",
    )
    .unwrap();
    harness.reload();
    harness.rendered("quota replacement healthy");
}

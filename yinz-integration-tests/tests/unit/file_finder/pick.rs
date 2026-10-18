use super::*;

#[test]
fn absolute_roots_work_from_an_exited_pane_but_relative_roots_require_a_cwd() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("held-picker.scm"),
        r#"
(define root "")
(define held #f)
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("RunCommands" "ReadApplicationState" "MessageAndLaunchOtherPlugins" "ChangeApplicationState"))
    (lambda (ignored)
      (set! root (hash-ref config "root"))
      (yinz-command (hash "Subscribe" '("CommandPaneExited")))
      (set! held (yinz-command (hash "OpenCommandPaneFloating"
        (list (hash "path" "/usr/bin/true") json-null (hash))))))))
(define (reject-relative options)
  (define pending (with-handler (lambda (failure) (yinz-reject! (yinz-deferred) failure))
    (yinz-pick (hash-insert options "kind" "file"))))
  (yinz-then
    (yinz-catch (yinz-then pending (lambda (ignored) #f))
      (lambda (failure)
        (unless (or (string-contains? failure "CWD") (string-contains? failure "not running")) (error failure)) #t))
    (lambda (rejected) (unless rejected (error "relative roots accepted without cwd")))))
(define (reject-relatives options)
  (if (null? options) (yinz-pick (hash "kind" "file" "roots" (list root)))
    (yinz-then (reject-relative (car options)) (lambda (ignored) (reject-relatives (cdr options))))))
(define (update event)
  (if (and held (hash? event) (hash-contains? event "CommandPaneExited")
           (= (car (hash-ref event "CommandPaneExited")) (hash-ref held "Terminal")))
    (yinz-catch
      (yinz-then #t (lambda (ignored)
        (unless (equal? (hash-ref (yinz-command "GetFocusedPaneInfo") "pane_id") held)
          (error "exited pane is not focused"))
        (yinz-then (reject-relatives (list (hash) (hash "roots" '(".")) (hash "roots" (list root "."))))
          (lambda (file)
            (unless (equal? file (string-append root "/book.pdf")) (error "wrong selection"))
            (unless (equal? (hash-ref (yinz-command "GetFocusedPaneInfo") "pane_id") held)
              (error "focus was not restored"))
            (yinz-command (hash "RenameTerminalPane" (list (hash-ref held "Terminal") "HELD PICK COMPLETE"))) #f))))
      (lambda (failure) (yinz-log (string-append "HELD PICK ERROR " failure)) #f))
    #f))
"#,
    )
    .unwrap();
    let path = root.path().join("held-picker.integration.test.scm");
    std::fs::write(&path, r#"
(test "absolute roots from an exited process"
  (hash "grants" '("RunCommands" "ReadApplicationState" "MessageAndLaunchOtherPlugins" "ChangeApplicationState"))
  (lambda ()
    (define home (test-session "fixture" (hash "files" (hash "project/book.pdf" "") "commands" (hash "fzf" "cat"))))
    (test-session "start")
    (test-wait-for-text "yinz-test>")
    (test-session "load" (hash "path" "./held-picker.scm" "headless" #t "configuration" (hash "root" (string-append home "/project"))))
    (test-wait-for "held pane pick completes" (lambda ()
      (define logs (test-session "logs"))
      (assert (not (string-contains? logs "HELD PICK ERROR")) logs)
      (not (null? (filter (lambda (pane)
        (and (equal? (hash-ref pane "title") "HELD PICK COMPLETE") (hash-ref pane "is_held"))) (test-session "panes"))))))))
"#).unwrap();
    let report = yinz_server::plugin_testing::run(
        &yinz_integration_tests::test_env::init().join("data"),
        &path,
        &Default::default(),
        None,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn awaited_folder_then_pdf_restores_focus_before_second_picker() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("picker.scm");
    std::fs::write(&source, r#"
(define (load config)
  (unless (hash-ref (yinz-command (hash "GetPluginInfo" json-null)) "background") (error "load provenance was lost"))
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState" "MessageAndLaunchOtherPlugins" "ChangeApplicationState"))
    (lambda (ignored)
      (define focused (hash-ref (yinz-command "GetFocusedPaneInfo") "pane_id"))
      (define (rename text)
        (when (hash-contains? focused "Terminal")
          (yinz-command (hash "RenameTerminalPane" (list (hash-ref focused "Terminal") text)))))
      (yinz-catch
        (yinz-then (yinz-pick (hash "kind" "directory" "roots" '("/tmp")))
          (lambda (folder)
            (when (json-null? folder) (error "first pick cancelled"))
            (yinz-then (yinz-pick (hash "kind" "file" "roots" (list folder) "extensions" '("pdf")))
              (lambda (file)
                (unless (equal? file "/tmp/Papers.pdf/quote\n雪.PDF") (error "path changed"))
                (rename "PICK COMPLETE")))))
        (lambda (failure)
          (yinz-log (string-append "PICK ERROR " failure))
          (rename (string-append "PICK ERROR " failure)))))))
(define (render rows columns) "PICKER BACKGROUND MUST NEVER SHOW")
"#).unwrap();
    let url = url::Url::from_file_path(source).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            r##"
pane_frame_style "full"
plugins {{
  picker location="{url}"
  file-finder location="yinz:file-finder" {{
    width "60%"
    fzf_args "--border"
    file_command "exit 77"
    openers_json r#"{{"default":{{"argv":["must-not-run"]}}}}"#
  }}
}}
load_plugins clear-defaults=true {{ "picker"; }}
"##
        ))
        .with_layout(yinz_types::data::LayoutInfo::Stringified(
            "layout { pane cwd=\"/tmp\"; }".into(),
        ))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("picker permissions", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("first picker opens", |grid| {
        assert!(!grid.contains("PICK ERROR"), "{grid}");
        grid.contains("Find file")
    });
    let first = yinz.expect_pty_spawn();
    let script = spawned_command(&first).1.remove(1);
    assert!(script.contains("--border"));
    assert!(!script.contains("exit 77"));
    assert!(!script.contains("must-not-run"));
    let out = |script: &str| {
        PathBuf::from(
            script
                .split_once("out='")
                .unwrap()
                .1
                .split_once('\'')
                .unwrap()
                .0,
        )
    };
    std::fs::write(out(&script), "/tmp/Papers.pdf\0").unwrap();
    first.exit(Some(0));
    let second = yinz.expect_pty_spawn();
    let script = spawned_command(&second).1.remove(1);
    assert!(script.contains("/tmp/Papers.pdf"));
    assert!(script.contains("-iname '*.pdf'"));
    second.output(b"SECOND PICKER READY");
    yinz.wait_until("second picker retains focus", |grid| {
        grid.contains("SECOND PICKER READY")
    });
    yinz.send_stdin(b"x");
    second.wait_for_stdin("input reaches the second picker", |bytes| {
        bytes.contains(&b'x')
    });
    std::fs::write(out(&script), "/tmp/Papers.pdf/quote\n雪.PDF\0").unwrap();
    second.exit(Some(0));
    yinz.wait_until("two awaited picks complete", |grid| {
        assert!(!grid.contains("PICK ERROR"), "{grid}");
        grid.contains("PICK COMPLETE")
    });
    assert!(
        !String::from_utf8_lossy(&yinz.raw_bytes()).contains("PICKER BACKGROUND MUST NEVER SHOW")
    );
    yinz.quit();
}

#[test]
fn focused_plugin_can_pick_then_query_plugin_ids_in_the_same_callback() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("focused-picker.scm");
    std::fs::write(&source, r#"
(define state "starting")
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState" "MessageAndLaunchOtherPlugins" "ChangeApplicationState"))
    (lambda (ignored)
      (yinz-command (hash "Subscribe" '("Key")))
      (set! state "READY TO PICK") (yinz-request-render))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "Key")
           (equal? (hash-ref (hash-ref event "Key") "bare_key") (hash "Char" "p")))
    (yinz-catch
      (yinz-then #t (lambda (ignored)
        (define pending (yinz-pick (hash "kind" "file")))
        (unless (= (length (yinz-command (hash "GetPluginIdsForUrl" "yinz:file-finder"))) 1)
          (error "picker driver missing"))
        (yinz-then pending (lambda (file)
          (unless (json-null? file) (error "expected cancellation"))
          (set! state "PICK AND QUERY COMPLETE") #t))))
      (lambda (failure) (set! state (string-append "PICK ERROR " failure)) #t))
    #f))
(define (render rows columns) state)
"#).unwrap();
    let url = url::Url::from_file_path(source).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .with_layout(yinz_types::data::LayoutInfo::Stringified(format!(
            "layout {{ pane cwd=\"{}\" {{ plugin location=\"{url}\"; }}; }}",
            root.path().display()
        )))
        .start();
    yinz.wait_until("focused picker permissions", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("focused plugin ready", |grid| {
        grid.contains("READY TO PICK")
    });
    yinz.send_stdin(b"p");
    let picker = yinz.expect_pty_spawn();
    let script = spawned_command(&picker).1.remove(1);
    let out = script
        .split_once("out='")
        .unwrap()
        .1
        .split_once('\'')
        .unwrap()
        .0;
    std::fs::write(out, "").unwrap();
    picker.exit(Some(0));
    yinz.wait_until("pick and synchronous query finish", |grid| {
        assert!(!grid.contains("PICK ERROR"), "{grid}");
        grid.contains("PICK AND QUERY COMPLETE")
    });
    yinz.quit();
}

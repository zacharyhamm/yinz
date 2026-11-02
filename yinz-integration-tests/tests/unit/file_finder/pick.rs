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
        (yinz-then (reject-relatives (list (hash) (hash "roots" '("."))))
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
    (define home (test-session "fixture" (hash "files" (hash "project/book.pdf" "") "commands" (hash "fzf" "exit 99"))))
    (test-session "start")
    (test-wait-for-text "yinz-test>")
    (test-session "load" (hash "path" "./held-picker.scm" "headless" #t "configuration" (hash "root" (string-append home "/project"))))
    (test-wait-for-text "book.pdf")
    (test-session "key" "b")
    (test-session "key" "Enter")
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
    let papers = root.path().join("Papers.pdf");
    std::fs::create_dir(&papers).unwrap();
    std::fs::write(papers.join("quote\n雪.PDF"), "").unwrap();
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
        (yinz-then (yinz-pick (hash "kind" "directory" "roots" (list (hash-ref config "root"))))
          (lambda (folder)
            (when (json-null? folder) (error "first pick cancelled"))
            (yinz-then (yinz-pick (hash "kind" "file" "roots" (list folder) "extensions" '("pdf")))
              (lambda (file)
                (unless (equal? file (string-append (hash-ref config "root") "/Papers.pdf/quote\n雪.PDF")) (error "path changed"))
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
  picker location="{url}" {{ root "{}"; }}
  file-finder location="yinz:file-finder" {{
    width "60%"
    fzf_args "--border"
    file_command "exit 77"
    openers_json r#"{{"default":{{"argv":["must-not-run"]}}}}"#
  }}
}}
load_plugins clear-defaults=true {{ "picker"; }}
"##,
            root.path().display()
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
        grid.contains("Papers.pdf/") && grid.contains("Ctrl+Enter")
    });
    assert_eq!(running_plugin(&yinz, "strider").pane, Some(true));
    assert_eq!(running_plugin(&yinz, "picker.scm").pane, Some(false));
    yinz.send_stdin(b"Papers\r");
    yinz.wait_until("folder opens", |grid| {
        grid.contains("Papers.pdf") && !grid.contains("Papers.pdf/") && grid.contains("Ctrl+Enter")
    });
    yinz.send_stdin(b"\x1b[13;5u");
    yinz.wait_until("second picker retains focus", |grid| {
        assert!(!grid.contains("PICK ERROR"), "{grid}");
        grid.contains("quote") && grid.contains("browse/select file")
    });
    yinz.send_stdin(b"quote\r");
    yinz.wait_until("two awaited picks complete", |grid| {
        assert!(!grid.contains("PICK ERROR"), "{grid}");
        grid.contains("PICK COMPLETE")
    });
    assert!(
        !String::from_utf8_lossy(&yinz.raw_bytes()).contains("PICKER BACKGROUND MUST NEVER SHOW")
    );
    // The picks were made by a plugin with no pane at all; it is still running, headless.
    assert_no_plugin_panes(&yinz, "after the picks");
    let picker = running_plugin(&yinz, "picker.scm");
    assert_eq!(picker.pane, Some(false), "{picker:#?}");
    yinz.quit();
}

#[test]
fn focused_plugin_can_pick_then_query_plugin_ids_in_the_same_callback() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("focused-picker.scm");
    std::fs::write(&source, r#"
(yinz-declare-pane)
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
        (unless (= (length (yinz-command (hash "GetPluginIdsForUrl" "yinz:strider"))) 1)
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
    yinz.wait_until("Strider opens", |grid| grid.contains("browse/select file"));
    yinz.send_stdin(b"\x1b");
    yinz.wait_until("pick and synchronous query finish", |grid| {
        assert!(!grid.contains("PICK ERROR"), "{grid}");
        grid.contains("PICK AND QUERY COMPLETE")
    });
    yinz.quit();
}

/// Every pane in the session, parked ones included, has to be a terminal.
fn assert_no_plugin_panes(yinz: &TestSession, when: &str) {
    let (code, json) = yinz.run_cli_action_reporting(yinz_cli::args::CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(code, 0, "{json}");
    let panes: Vec<yinz_types::data::PaneListEntry> = serde_json::from_str(&json).unwrap();
    assert!(
        panes.iter().all(|pane| !pane.pane_info.is_plugin()),
        "{when}: a picking plugin or its driver has a pane: {panes:#?}"
    );
}

/// The running instance whose location ends with `location`.
fn running_plugin(yinz: &TestSession, location: &str) -> yinz_types::cli::PluginStatus {
    let plugins = yinz_client::store_client::plugin_request(
        &yinz.session_name().into(),
        yinz_types::cli::PluginRequest {
            operation: yinz_types::cli::PluginOperation::List,
            target: None,
            configuration: None,
            fallback: None,
            candidates: vec![],
            caller: None,
        },
    )
    .unwrap()
    .plugins;
    let status = plugins
        .iter()
        .find(|status| status.location.ends_with(location))
        .unwrap_or_else(|| panic!("{location} is not running: {plugins:#?}"))
        .clone();
    assert_eq!(
        status.state,
        yinz_types::cli::PluginState::Running,
        "{status:#?}"
    );
    status
}

fn assert_centered_wizard(yinz: &TestSession, size: yinz_types::pane_size::Size) {
    let (code, json) = yinz.run_cli_action_reporting(yinz_cli::args::CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: true,
        all: true,
        json: true,
    });
    assert_eq!(code, 0, "{json}");
    let panes: Vec<yinz_types::data::PaneListEntry> = serde_json::from_str(&json).unwrap();
    let pane = &panes
        .iter()
        .find(|pane| pane.pane_info.title.starts_with("First Run Setup Wizard"))
        .unwrap()
        .pane_info;
    let (x, y) = (pane.pane_x.get(), pane.pane_y.get());
    let (cols, rows) = (pane.pane_columns.get(), pane.pane_rows.get());
    assert_eq!(cols, 96.min(size.cols - 2), "{pane:#?}");
    assert_eq!(rows, 32.min(size.rows - 2), "{pane:#?}");
    assert!(x.abs_diff(size.cols - x - cols) <= 1, "{pane:#?}");
    assert!(y.abs_diff(size.rows - y - rows) <= 1, "{pane:#?}");
}

fn resize_wizard(yinz: &TestSession) {
    for (cols, rows) in [
        (80, 24),
        (120, 40),
        (60, 20),
        (40, 12),
        (20, 8),
        (60, 20),
        (120, 40),
    ] {
        let size = yinz_types::pane_size::Size { cols, rows };
        yinz.resize(size);
        yinz.wait_until("resized wizard control and help", |grid| {
            grid.row_count() == rows
                && grid.styles.first().is_some_and(|row| row.len() == cols)
                && grid.contains("Enter")
        });
        assert_centered_wizard(yinz, size);
    }
}

#[test]
fn first_run_wizard_browses_selects_cancels_and_reopens_strider() {
    let config = tempfile::tempdir().unwrap();
    // The wizard starts from the session cwd, independently of its pane's cwd.
    let root = yinz_integration_tests::test_env::init().join("cwd");
    let project = root.join("chosen-project");
    std::fs::create_dir(&project).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .with_config_dir(config.path().to_owned())
        .with_layout(yinz_types::data::LayoutInfo::Stringified(format!(
            r#"layout {{
                pane
                floating_panes {{
                    pane cwd="{}" {{
                        plugin location="yinz:configuration" {{ is_setup_wizard "true"; }}
                    }}
                }}
            }}"#,
            root.display()
        )))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("first-run preset wizard", |grid| grid.contains("Step 1/3"));
    assert_centered_wizard(&yinz, TERMINAL_SIZE);
    yinz.send_stdin(b"\x1b[B");
    resize_wizard(&yinz);
    // A deliberate move is reset by the next terminal resize.
    yinz.run_cli_action(yinz_cli::args::CliAction::MovePane {
        direction: Some(yinz_types::geometry::Direction::Right),
        pane_id: None,
    });
    resize_wizard(&yinz);
    yinz.send_stdin(b"\r");
    yinz.wait_until("first project form", |grid| {
        grid.contains("Choose your first project")
    });
    resize_wizard(&yinz);
    yinz.send_stdin(b"\tresize draft\t\t\t");
    yinz.send_stdin(b"\x06");
    yinz.wait_until("Strider folder browser", |grid| {
        grid.contains("chosen-project/") && grid.contains("Ctrl+Enter")
    });
    for (cols, rows) in [(60, 20), (40, 12), (120, 40)] {
        let size = yinz_types::pane_size::Size { cols, rows };
        yinz.resize(size);
        yinz.wait_until("browser remains active across resize", |grid| {
            grid.row_count() == rows && grid.contains("chosen-project/")
        });
        assert_centered_wizard(&yinz, size);
    }
    yinz.send_stdin(b"chosen\r");
    yinz.wait_until("chosen folder opens", |grid| {
        grid.contains("chosen-project") && !grid.contains("chosen-project/")
    });
    yinz.send_stdin(b"\x1b[13;5u");
    yinz.wait_until("selection returns to wizard", |grid| {
        grid.contains("Choose your first project") && grid.contains("chosen-project")
    });
    yinz.send_stdin(b"\x06");
    yinz.wait_until("browser reopens at selected folder", |grid| {
        grid.contains("chosen-project") && grid.contains("Ctrl+Enter")
    });
    yinz.send_stdin(b"\x1b");
    yinz.wait_until("dismissal preserves selected path", |grid| {
        grid.contains("Choose your first project") && grid.contains("chosen-project")
    });
    resize_wizard(&yinz);
    yinz.wait_until("resizing preserves the project name draft", |grid| {
        grid.contains("resize draft")
    });
    yinz.send_stdin(b"\x06");
    yinz.wait_until("browser opens again after dismissal", |grid| {
        grid.contains("Ctrl+Enter")
    });
    yinz.send_stdin(b"\x1b");
    yinz.wait_until("wizard survives repeated browsing", |grid| {
        grid.contains("Choose your first project")
    });
    yinz.quit();
}

#[test]
fn first_run_wizard_renders_at_its_centered_size_before_any_terminal_resize() {
    let config = tempfile::tempdir().unwrap();
    let size = yinz_types::pane_size::Size { cols: 90, rows: 51 };
    let mut yinz = TestRunner::new(size)
        .with_config(NO_BACKGROUND_PLUGINS)
        .with_config_dir(config.path().to_owned())
        .with_layout(yinz_types::data::LayoutInfo::Stringified(
            "layout { pane; floating_panes { pane { plugin location=\"yinz:configuration\" { is_setup_wizard \"true\"; }; }; }; }".into()
        ))
        .start();
    yinz.expect_pty_spawn().output(PROMPT);
    // The 32-row pane has a 30-row content area. Help belongs on its last row,
    // not the last row of the smaller rectangle used to allocate the pane.
    let top = (size.rows - 32) / 2;
    yinz.wait_until(
        "preset uses the final content size before resizing",
        |grid| grid.row_of_line("Enter") == Some(top + 30) && grid.contains("1. Default"),
    );
    assert_centered_wizard(&yinz, size);
    yinz.send_stdin(b"\r");
    let project = yinz.wait_until("project content before resizing", |grid| {
        grid.contains("Choose your first project")
    });
    let title = "Choose your first project";
    let title_row = project
        .text
        .lines()
        .find(|line| line.contains(title))
        .unwrap();
    let x = title_row[..title_row.find(title).unwrap()].chars().count();
    assert!(x.abs_diff(size.cols - x - title.len()) <= 1, "{project}");
    assert!(project.row_of_line(title).unwrap() > top + 5, "{project}");
    yinz.quit();
}

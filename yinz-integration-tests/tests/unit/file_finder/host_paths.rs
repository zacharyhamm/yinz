use super::*;
use yinz_types::data::LayoutInfo;

fn check_host_paths(custom_opener: bool) {
    let root = tempfile::Builder::new()
        .prefix("finder-")
        .tempdir_in("/tmp")
        .unwrap();
    let cwd = root.path().join("project");
    let chosen = cwd.join("sub/book, one.pdf");
    std::fs::create_dir_all(chosen.parent().unwrap()).unwrap();
    let opener = if custom_opener {
        "open_pdf \"/tmp/my-viewer --literal {}\""
    } else {
        ""
    };
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!("{NO_BACKGROUND_PLUGINS}plugins {{\n file-finder location=\"yinz:file-finder\" cwd=\"{}\" {{\n {opener}\n }}\n}}", root.path().display()))
        .with_layout(LayoutInfo::Stringified(format!("layout {{ pane cwd=\"{}\"; }}", cwd.display())))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell prompt is visible", |screen| screen.contains("$ "));
    let (finder, selection_file) = open_finder(&yinz);
    match finder.terminal_action() {
        Some(TerminalAction::RunCommand(command)) => assert_eq!(command.cwd.as_ref(), Some(&cwd)),
        other => panic!("expected finder command: {other:?}"),
    }
    choose(&finder, &selection_file, chosen.to_str().unwrap());
    let viewer = yinz.expect_pty_spawn();
    match viewer.terminal_action() {
        Some(TerminalAction::RunCommand(command)) if custom_opener => {
            assert_eq!(command.cwd.as_deref(), chosen.parent());
            assert_eq!(command.command, Path::new("/tmp/my-viewer"));
            assert_eq!(command.args, ["--literal", chosen.to_str().unwrap()]);
        },
        Some(TerminalAction::OpenFile(file)) if !custom_opener => {
            assert_eq!(file.path, chosen);
            assert_eq!(file.cwd.as_deref(), chosen.parent());
        },
        other => panic!("unexpected viewer: {other:?}"),
    }
    viewer.exit(Some(0));
    yinz.quit();
}

#[test]
fn finder_and_custom_opener_keep_host_cwds_and_literal_arguments() {
    check_host_paths(true);
}

#[test]
fn finder_and_editor_keep_host_paths() {
    check_host_paths(false);
}

#[test]
fn real_finder_and_editor_use_the_tmp_project() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("finder.integration.test.scm");
    std::fs::write(&path, r#"
(for-each (lambda (launch)
  (test (string-append "finder host paths through real PTYs: " launch) (hash "grants" '("RunCommands"))
    (lambda ()
      (define home (test-session "fixture" (hash
        "files" (hash "project/sub/book, #?%23%2523.pdf" "finder-host-path-sentinel")
        "commands" (hash
          "fzf" "set -eu
expected=$(cd \"$HOME/project\" && pwd -P)
[ \"$(pwd -P)\" = \"$expected\" ] || exit 1
input=$(cat)
[ \"$input\" = 'sub/book, #?%23%2523.pdf' ] || exit 2
printf '%s\\n' \"$HOME/project/sub/book, #?%23%2523.pdf\"
"
          "vi" "set -eu
[ \"$#\" -eq 1 ] || exit 3
[ \"$1\" = \"$HOME/project/sub/book, #?%23%2523.pdf\" ] || exit 4
[ \"$(pwd -P)\" = \"$(cd \"$HOME/project/sub\" && pwd -P)\" ] || exit 5
printf 'EDITOR_CWD=%s\\nEDITOR_ARG=%s\\n' \"$(pwd -P)\" \"$1\"
cat \"$1\"
printf '\\nEDITOR_READY\\n'
read release
"))))
      (test-session "start" (hash "columns" 240))
      (test-wait-for-text "yinz-test>")
      (test-session "text" "cd \"$HOME/project\" && printf 'PROJECT_%s\\n' READY")
      (test-session "key" "Enter")
      (test-wait-for-text "PROJECT_READY")
      (if (equal? launch "keyboard") (test-session "key" "Alt e")
        (test-session "load" (hash "path" "yinz:file-finder" "configuration"
          (hash "file_command" "printf '%s\\n' 'sub/book, #?%23%2523.pdf'" "open_pdf" "vi {}"))))
      (test-wait-for-text "EDITOR_READY")
      (define screen (test-session "screen"))
      (assert (string-contains? screen (string-append "EDITOR_CWD=" home "/project/sub")))
      (assert (string-contains? screen (string-append "EDITOR_ARG=" home "/project/sub/book, #?%23%2523.pdf")))
      (assert (string-contains? screen "finder-host-path-sentinel"))
      (test-session "key" "Enter")
      (test-wait-for "finder and editor cleaned up" (lambda ()
        (define panes (test-session "panes"))
        (and (= (length (filter (lambda (pane) (not (hash-ref pane "is_plugin"))) panes)) 1)
          (null? (filter (lambda (pane) (equal? (hash-ref pane "plugin_url") "yinz:file-finder")) panes))
          (not (string-contains? (test-session "screen") "EDITOR_READY"))))))))
  '("keyboard" "typed"))
"#).unwrap();
    let report = yinz_server::plugin_testing::run(
        &path,
        &Default::default(),
        None,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

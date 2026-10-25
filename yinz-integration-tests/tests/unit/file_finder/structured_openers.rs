use super::*;
use yinz_cli::args::CliAction;
use yinz_types::data::{PaneId, PaneListEntry};

fn panes(yinz: &TestSession) -> Vec<PaneListEntry> {
    let (status, json) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(status, 0, "{json}");
    serde_json::from_str(&json).unwrap()
}

fn is_reader(pane: &PaneListEntry, url: &str) -> bool {
    let display = format!(
        "file:{}",
        url::Url::parse(url)
            .unwrap()
            .to_file_path()
            .unwrap()
            .canonicalize()
            .unwrap()
            .display()
    );
    matches!(pane.pane_info.plugin_url.as_deref(), Some("reader"))
        || pane.pane_info.plugin_url.as_deref() == Some(display.as_str())
}

fn receiver(root: &Path) -> String {
    let path = root.join("reader '雪,%2C.scm");
    std::fs::write(
        &path,
        r#"
(yinz-declare-pane)
(define ready (yinz-deferred))
(define payload "")
(define source "")
(define count 0)
(define settings "")
(define (load config)
  (set! settings (if (hash-contains? config "fit") (hash-ref config "fit") ""))
  (yinz-command (hash "Subscribe" '("Key")))
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions '("ChangeApplicationState"))
      (lambda (ignored) (yinz-resolve! ready #t)))
    (lambda (failure) (yinz-resolve! ready #f))))
(define (pipe message)
  (yinz-then ready (lambda (granted)
    (if granted
      (begin
        (set! payload (hash-ref message "payload"))
        (set! source (yinz-json-string (hash-ref message "source")))
        (set! count (+ count 1)))
      (set! payload "DENIED"))
    #t)))
(define (update event)
  (when (and (hash? event) (hash-contains? event "Key")
             (equal? (hash-ref (hash-ref event "Key") "bare_key") (hash "Char" "h")))
    (yinz-command "HideSelf")) #t)
(define (render rows columns)
  (string-append "RECEIVED " (number->string count) " " settings "\n" payload "\nSOURCE " source))
"#,
    )
    .unwrap();
    url::Url::from_file_path(&path).unwrap().to_string()
}

fn session(json: &str, aliases: &str) -> TestSession {
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!("{NO_BACKGROUND_PLUGINS}plugins {{\n{aliases}\nfile-finder location=\"yinz:file-finder\" {{\nopeners_json r#\"{json}\"#\n}}\n}}"))
        .start();
    yinz.expect_pty_spawn().output(PROMPT);
    yinz.wait_for_app_load();
    yinz
}

fn assert_retired(yinz: &TestSession, finder: &FakePtyHandle, selection: &Path) {
    yinz.wait_until("finder driver and overlay retired", |_| {
        !selection.exists()
            && panes(yinz).iter().all(|p| {
                p.pane_info.id != PaneId::Terminal(finder.terminal_id())
                    && p.pane_info.plugin_url.as_deref() != Some("file-finder")
            })
    });
}

#[test]
fn plugin_handoff_survives_permissions_and_reuses_hidden_reader() {
    let root = tempfile::tempdir().unwrap();
    let url = receiver(root.path());
    let mut yinz = session(
        r#"{"pdf":{"plugin":"reader"}}"#,
        &format!("reader location={url:?} {{ fit \"width\"; }}"),
    );
    let chosen = "/tmp/a '雪,\"#?%2C.pdf";
    let (finder, selection) = open_finder(&yinz);
    // The driver declares no pane and runs headless; its plugin id names its selection file.
    assert!(!panes(&yinz)
        .iter()
        .any(|p| p.pane_info.plugin_url.as_deref() == Some("file-finder")));
    let driver = selection
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.rsplit('-').next())
        .unwrap()
        .to_owned();
    choose(&finder, &selection, chosen);
    yinz.wait_until("reader asks for its own permissions", |g| {
        g.contains("Plugin permissions")
    });
    assert_retired(&yinz, &finder, &selection);
    yinz.send_stdin(b"\t\r");
    yinz.wait_until(
        "reader receives exact raw path after permission grant",
        |g| g.contains(chosen) && g.contains("RECEIVED 1 width"),
    );
    let reader = panes(&yinz)
        .into_iter()
        .find(|p| is_reader(p, &url))
        .unwrap();
    assert!(
        reader.pane_info.is_focused
            && reader.pane_info.is_floating
            && !reader.pane_info.is_suppressed
    );
    assert!(yinz
        .snapshot()
        .contains(&format!("SOURCE {{\"Plugin\":{driver}}}")));
    assert_eq!(
        panes(&yinz)
            .iter()
            .filter(|p| !p.pane_info.is_plugin())
            .count(),
        1
    );
    assert_eq!(
        yinz.run_cli_action(CliAction::HideFloatingPanes {
            tab_id: Some(reader.tab_id)
        }),
        0
    );
    let (finder, selection) = open_finder(&yinz);
    choose(&finder, &selection, "/tmp/second.pdf");
    yinz.wait_until("hidden reader is revealed with second payload", |g| {
        g.contains("RECEIVED 2 width") && g.contains("/tmp/second.pdf")
    });
    assert_retired(&yinz, &finder, &selection);
    let readers: Vec<_> = panes(&yinz)
        .into_iter()
        .filter(|p| is_reader(p, &url))
        .collect();
    assert_eq!(readers.len(), 1);
    assert_eq!(readers[0].pane_info.id, reader.pane_info.id);
    assert!(readers[0].pane_info.is_focused);
    yinz.send_stdin(b"h");
    yinz.wait_until("reader suppresses itself", |_| {
        panes(&yinz)
            .iter()
            .any(|p| p.pane_info.id == reader.pane_info.id && p.pane_info.is_suppressed)
    });
    let (finder, selection) = open_finder(&yinz);
    choose(&finder, &selection, "/tmp/suppressed.pdf");
    yinz.wait_until("suppressed reader is revealed", |g| {
        g.contains("RECEIVED 3 width") && g.contains("/tmp/suppressed.pdf")
    });
    assert_retired(&yinz, &finder, &selection);
    assert_eq!(
        yinz.run_cli_action(CliAction::NewTab {
            layout: None,
            layout_string: Some("layout { pane; }".into()),
            layout_dir: None,
            name: Some("elsewhere".into()),
            cwd: None,
            initial_command: vec![],
            initial_plugin: None,
            close_on_exit: false,
            start_suspended: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            no_focus: false,
        }),
        0
    );
    yinz.expect_pty_spawn().output(b"OTHER TAB READY\r\n");
    yinz.wait_until("another tab is active", |g| g.contains("OTHER TAB READY"));
    let (finder, selection) = open_finder(&yinz);
    choose(&finder, &selection, "/tmp/other-tab.pdf");
    yinz.wait_until("reader's original tab is focused", |g| {
        g.contains("RECEIVED 4 width") && g.contains("/tmp/other-tab.pdf")
    });
    assert_retired(&yinz, &finder, &selection);
    assert!(panes(&yinz)
        .iter()
        .any(|p| p.pane_info.id == reader.pane_info.id
            && p.tab_id == reader.tab_id
            && p.pane_info.is_focused));
    assert_eq!(
        yinz.run_cli_action(CliAction::ClosePane {
            pane_id: Some(reader.pane_info.id.to_string())
        }),
        0
    );
    yinz.wait_until("reader closes normally", |_| {
        !panes(&yinz)
            .iter()
            .any(|p| p.pane_info.id == reader.pane_info.id)
    });
    let (finder, selection) = open_finder(&yinz);
    finder.exit(Some(130));
    assert_retired(&yinz, &finder, &selection);
    yinz.quit();
}

#[test]
fn malformed_configuration_and_unknown_alias_retire_the_finder() {
    for json in [
        r#"{"pdf":{"argv":[]}}"#,
        r#"{"pdf":{"plugin":"missing-reader-alias"}}"#,
    ] {
        let mut yinz = session(json, "");
        let (finder, selection) = open_finder(&yinz);
        choose(&finder, &selection, "/tmp/test.pdf");
        yinz.wait_until("opening failure is visible", |g| g.contains("file-finder:"));
        assert_retired(&yinz, &finder, &selection);
        assert_eq!(
            panes(&yinz)
                .iter()
                .filter(|p| !p.pane_info.is_plugin())
                .count(),
            1
        );
        yinz.quit();
    }
}

#[test]
fn structured_argv_reaches_the_terminal_without_shell_parsing() {
    let mut yinz = session(
        r#"{"pdf":{"argv":["/opt/Image Tools/viewer", "", "a 'quote'", "{}:{}"]}}"#,
        "",
    );
    let (finder, selection) = open_finder(&yinz);
    let chosen = "/tmp/ '雪,\"#?%2C.pdf";
    choose(&finder, &selection, chosen);
    let viewer = yinz.expect_pty_spawn();
    assert_eq!(
        spawned_command(&viewer),
        (
            "/opt/Image Tools/viewer".into(),
            vec!["".into(), "a 'quote'".into(), format!("{chosen}:{chosen}")]
        )
    );
    viewer.exit(Some(0));
    assert_retired(&yinz, &finder, &selection);
    yinz.quit();
}

#[test]
fn reader_permission_denial_and_missing_source_leave_no_finder() {
    let root = tempfile::tempdir().unwrap();
    let url = receiver(root.path());
    let json = serde_json::json!({"pdf": {"plugin": url}}).to_string();
    let mut yinz = session(&json, "");
    let (finder, selection) = open_finder(&yinz);
    choose(&finder, &selection, "/tmp/denied.pdf");
    yinz.wait_until("reader permission prompt", |g| {
        g.contains("Plugin permissions")
    });
    assert_retired(&yinz, &finder, &selection);
    yinz.send_stdin(b"\x1b");
    yinz.wait_until("receiver handles denial", |g| g.contains("DENIED"));
    yinz.quit();

    let missing = root.path().join("missing.scm");
    let json = serde_json::json!({"pdf": {"plugin": missing}}).to_string();
    let mut yinz = session(&json, "");
    let (finder, selection) = open_finder(&yinz);
    choose(&finder, &selection, "/tmp/missing.pdf");
    assert_retired(&yinz, &finder, &selection);
    // A reader whose source cannot be read gets no pane to paint its error into. The host
    // reports the load failure and the finder its failed handoff, both as notifications.
    yinz.wait_until("missing reader load error is visible", |g| {
        g.contains("failed to open reader") && g.contains("to find the plugin")
    });
    assert!(!panes(&yinz).iter().any(|pane| pane
        .pane_info
        .plugin_url
        .as_deref()
        .is_some_and(|url| url.ends_with("missing.scm"))));
    yinz.quit();
}

#[test]
fn alias_settings_narrow_reuse_and_lowest_matching_id_wins() {
    let root = tempfile::tempdir().unwrap();
    let url = receiver(root.path());
    let json = serde_json::json!({"pdf":{"plugin":"reader"},"txt":{"plugin":url}}).to_string();
    let mut yinz = session(
        &json,
        &format!("reader location={url:?} {{ fit \"width\"; }}"),
    );
    let mut ids = vec![];
    for fit in ["height", "width", "width"] {
        assert_eq!(
            yinz.run_cli_action(CliAction::LaunchPlugin {
                floating: true,
                in_place: false,
                close_replaced_pane: false,
                url: url.parse().unwrap(),
                configuration: None,
                configuration_json: Some(yinz_types::input::layout::PluginUserConfiguration::new(
                    std::collections::BTreeMap::from([("fit".into(), fit.into())])
                )),
                skip_plugin_cache: false,
                no_focus: false,
                tab_id: None,
            }),
            0
        );
        if ids.is_empty() {
            yinz.wait_until("first reader permission prompt", |g| {
                g.contains("Plugin permissions")
            });
            yinz.send_stdin(b"\t\r");
        }
        yinz.wait_until("new reader ready", |g| {
            g.contains(&format!("RECEIVED 0 {fit}"))
                && panes(&yinz).iter().filter(|p| is_reader(p, &url)).count() == ids.len() + 1
        });
        ids.push(
            panes(&yinz)
                .into_iter()
                .filter(|p| is_reader(p, &url))
                .map(|p| p.pane_info.id)
                .max()
                .unwrap(),
        );
    }
    for (path, expected, fit) in [
        ("/tmp/alias.pdf", ids[1], "width"),
        ("/tmp/any.txt", ids[0], "height"),
    ] {
        let (finder, selection) = open_finder(&yinz);
        choose(&finder, &selection, path);
        yinz.wait_until("exactly one matching reader receives the path", |g| {
            g.contains(path) && g.contains(&format!("RECEIVED 1 {fit}"))
        });
        assert_retired(&yinz, &finder, &selection);
        let matching: Vec<_> = panes(&yinz)
            .into_iter()
            .filter(|p| is_reader(p, &url))
            .collect();
        assert_eq!(matching.len(), 3);
        assert!(matching
            .iter()
            .any(|p| p.pane_info.id == expected && p.pane_info.is_focused));
    }
    yinz.quit();
}

#[test]
fn background_handoff_never_renders_the_destination_and_keeps_palette_registration() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("background.scm");
    std::fs::write(
        &source,
        r#"
(define terminal 0)
(define (background?) (hash-ref (yinz-command (hash "GetPluginInfo" json-null)) "background"))
(define (load config)
  (unless (background?) (error "background provenance missing during load"))
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState" "ChangeApplicationState"))
    (lambda (ignored)
      (unless (json-null? (yinz-command (hash "GetPluginInfo" 4294967295)))
        (error "missing instance must return null"))
      (define pane (hash-ref (yinz-command "GetFocusedPaneInfo") "pane_id"))
      (when (hash-contains? pane "Terminal") (set! terminal (hash-ref pane "Terminal")))
      (yinz-palette-register "handoff" "Handoff"
        (list (hash "id" "alive" "label" "Background handoff still registered"
          "run" (lambda (values context) (void)))))
      (yinz-command (hash "RenameTerminalPane" (list terminal "BACKGROUND READY"))))))
(define (pipe message)
  (unless (background?) (error "background provenance changed"))
  (yinz-command (hash "RenameTerminalPane"
    (list terminal (string-append "BACKGROUND DELIVERED " (hash-ref message "payload"))))))
(define (render rows columns) "BACKGROUND DESTINATION MUST NEVER APPEAR")
"#,
    )
    .unwrap();
    let url = url::Url::from_file_path(source).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            r##"
plugins {{
 reader location="{url}"
 file-finder location="yinz:file-finder" {{ openers_json r#"{{"pdf":{{"plugin":"reader"}}}}"#; }}
}}
load_plugins clear-defaults=true {{ "reader"; }}
"##
        ))
        .start();
    yinz.expect_pty_spawn().output(PROMPT);
    yinz.wait_until("background reader permissions", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("background reader initialized", |grid| {
        grid.contains("BACKGROUND READY")
    });
    // The reader declares no pane, so it runs headless with no pane record at all.
    let no_reader_pane = |yinz: &TestSession| {
        assert!(!panes(yinz).iter().any(|pane| is_reader(pane, url.as_str())));
    };
    no_reader_pane(&yinz);
    let (finder, selection) = open_finder(&yinz);
    choose(&finder, &selection, "/tmp/paper.pdf");
    yinz.wait_until("background reader receives path", |grid| {
        grid.contains("BACKGROUND DELIVERED")
    });
    assert_retired(&yinz, &finder, &selection);
    no_reader_pane(&yinz);
    assert!(!String::from_utf8_lossy(&yinz.raw_bytes())
        .contains("BACKGROUND DESTINATION MUST NEVER APPEAR"));
    yinz.send_stdin(b"\x1b[112;7u");
    yinz.send_stdin(b"Background handoff still registered");
    yinz.wait_until("registration survives handoff", |grid| {
        grid.contains("Background handoff still registered")
    });
    yinz.quit();
}

#[test]
fn message_only_missing_reader_fails_without_a_cold_launch() {
    let root = tempfile::tempdir().unwrap();
    let url = receiver(root.path());
    let mut yinz = session(
        r#"{"pdf":{"plugin":"reader","delivery":"message"}}"#,
        &format!("reader location={url:?}"),
    );
    let (finder, selection) = open_finder(&yinz);
    choose(&finder, &selection, "/tmp/paper.pdf");
    yinz.wait_until("message-only failure is reported", |grid| {
        grid.contains("message destination is not") && grid.contains("running")
    });
    assert_retired(&yinz, &finder, &selection);
    assert!(panes(&yinz).iter().all(|pane| !is_reader(pane, &url)));
    yinz.quit();
}

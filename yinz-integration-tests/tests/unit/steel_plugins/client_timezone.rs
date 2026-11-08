use super::*;

const PANE: &str = r#"
(yinz-declare-pane)
(define state "timezone starting")
(define zones (hash))
(define listing "")
(define (summary)
  (string-join (map (lambda (id) (yinz-json-string (hash-ref zones id))) (hash-keys->list zones)) ","))
(define (query)
  (set! state (string-append "TZ actor " (yinz-json-string (yinz-client-timezone)) " "
    (yinz-json-string (yinz-client-utc-offset 1767225600000)) " "
    (yinz-json-string (yinz-client-utc-offset 1782864000000))))
  (yinz-command "ListClients") #t)
(define (load config)
  (yinz-on-event "ClientTimezoneChanged" (lambda (change)
    (set! zones (hash-insert zones (hash-ref change "client_id") (hash-ref change "timezone"))) #t))
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState")) (lambda (_)
    (yinz-on-event "ListClients" (lambda (clients)
      (set! listing (string-join (map (lambda (client) (yinz-json-string (hash-ref client "timezone"))) clients) ",")) #t))
    (yinz-command (hash "Subscribe" '("Key"))) (query))))
(define (update event) (when (hash-contains? event "Key") (query)) #t)
(define (render rows cols) (string-append state "\nlisted " listing "\nzones " (summary)))
"#;

#[test]
fn client_timezone_multiclient_queries_listing_disconnect_and_late_headless_palette() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("timezone.scm");
    fs::write(&plugin, PANE).unwrap();
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_timezone(Some("America/Chicago"))
        .with_config("load_plugins clear-defaults=true {}\nmirror_session false")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ pane {{ plugin location=\"file:{}\" {{ shared true; }}; }}; }}",
            plugin.display()
        )))
        .start();
    session.wait_until("timezone listing permission prompt", |grid| {
        grid.contains("Plugin permissions")
    });
    session.send_stdin(b"\t\r");
    session.wait_until("Chicago query and listing", |grid| {
        grid.contains("TZ actor \"America/Chicago\" -21600 -18000")
            && grid.contains("listed \"America/Chicago\"")
    });
    let other = session.attach_client_with_timezone(TERMINAL_SIZE, Some("Asia/Kolkata"));
    other.wait_until("Kolkata attachment broadcast", |grid| {
        grid.contains("zones") && grid.contains("Asia/Kolkata")
    });
    other.send_stdin(b"q");
    other.wait_until("acting Kolkata query and both listing zones", |grid| {
        grid.contains("TZ actor \"Asia/Kolkata\" 19800 19800")
            && grid.lines().iter().any(|line| {
                line.contains("listed")
                    && line.contains("America/Chicago")
                    && line.contains("Asia/Kolkata")
            })
    });
    // Start the headless provider after both attachments to exercise subscription snapshots.
    let provider = directory.path().join("provider.scm");
    fs::write(
        &provider,
        r#"
(define zones (hash))
(define (load config)
  (yinz-on-event "ClientTimezoneChanged" (lambda (change)
    (set! zones (hash-insert zones (hash-ref change "client_id") (hash-ref change "timezone")))))
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState")) (lambda (_)
    (yinz-palette-register "timezone" "Timezone" (lambda (query context)
      (list (hash "id" "tz" "label" (string-append "tzprovider " (hash-ref context "timezone") " "
        (yinz-client-timezone) " " (number->string (length (hash-keys->list zones))))
        "run" (lambda (_ execution)
          (hash "type" "text" "title" "TZ execution" "text" (hash-ref execution "timezone"))))))))))
"#,
    )
    .unwrap();
    let response = lifecycle(
        &session,
        yinz_types::cli::PluginOperation::Start,
        &provider,
        Some(std::collections::BTreeMap::from([(
            "shared".into(),
            "true".into(),
        )])),
    );
    assert!(!response.plugins.is_empty(), "{response:?}");
    session.wait_until("headless listing permission prompt", |grid| {
        grid.contains("Plugin permissions")
    });
    session.send_stdin(b"\t\r");
    other.send_stdin(b"\x1b[112;7u");
    other.send_stdin(b"tzprovider");
    other.wait_until(
        "headless provider snapshot and Kolkata palette discovery",
        |grid| grid.contains("tzprovider Asia/Kolkata Asia/Kolkata 2"),
    );
    other.send_stdin(b"\r");
    other.wait_until("palette execution timezone", |grid| {
        grid.contains("TZ execution") && grid.contains("Asia/Kolkata")
    });
    other.send_stdin(b"\x1b");
    other.quit();
    session.wait_until("disconnected subject invalidation", |grid| {
        grid.lines().iter().any(|line| {
            line.contains("zones") && line.contains("null") && !line.contains("Asia/Kolkata")
        })
    });
    let missing = session.attach_client_with_timezone(TERMINAL_SIZE, None);
    missing.wait_until("missing-report reattachment", |grid| grid.contains("zones"));
    missing.send_stdin(b"q");
    missing.wait_until("unknown client query has no server fallback", |grid| {
        grid.contains("TZ actor null null null")
    });
    missing.quit();
    session.quit();
}

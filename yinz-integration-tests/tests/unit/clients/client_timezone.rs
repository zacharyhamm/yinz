use super::*;

#[test]
fn client_timezone_prepared_session_switch_uses_the_switching_clients_report() {
    for report in [Some("America/Chicago"), None] {
        let directory = tempfile::tempdir().unwrap();
        let plugin = directory.path().join("destination.scm");
        std::fs::write(&plugin, r#"
(yinz-declare-pane)
(define zones (hash))
(define (load config)
  (yinz-on-event "ClientTimezoneChanged" (lambda (change)
    (set! zones (hash-insert zones (hash-ref change "client_id") (hash-ref change "timezone"))) #t)))
(define (render rows cols)
  (string-append "DEST TIMEZONES " (yinz-json-string (map (lambda (id) (hash-ref zones id)) (hash-keys->list zones)))))
"#).unwrap();
        let mut source = TestRunner::new(TERMINAL_SIZE)
            .with_timezone(report)
            .with_config("load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }\ndisable_session_metadata false")
            .with_layout(LayoutInfo::Stringified("layout { pane; }".into())).start();
        source.expect_pty_spawn().output(b"SOURCE TIMEZONE READY");
        source.wait_until("source attached", |grid| {
            grid.contains("SOURCE TIMEZONE READY")
        });
        let mut destination = TestRunner::new(TERMINAL_SIZE)
            .skip_concurrency_slot()
            .with_timezone(Some("Asia/Kolkata"))
            .with_config("load_plugins clear-defaults=true {}\ndisable_session_metadata false")
            .with_layout(LayoutInfo::Stringified(format!(
                "layout {{ pane {{ plugin location=\"file:{}\" {{ shared true; }}; }}; }}",
                plugin.display()
            )))
            .start();
        destination.wait_until("destination initial timezone snapshot", |grid| {
            grid.contains("DEST TIMEZONES") && grid.contains("Asia/Kolkata")
        });
        source.send_stdin(b"\x1b[112;7u");
        source.wait_until("session palette open", |grid| {
            grid.contains("Do · Project:")
        });
        source.send_stdin("Switch session…".as_bytes());
        source.wait_until("session action selected", |grid| {
            grid.contains("› Switch session…")
        });
        source.send_stdin(&keys::ENTER);
        source.wait_until("session picker ready", |grid| {
            grid.contains("Esc Back") && !grid.contains("Working")
        });
        source.send_stdin(destination.session_name().as_bytes());
        source.wait_until("destination offered", |grid| {
            grid.contains(&format!("› Local / {}", destination.session_name()))
                && grid.contains("Running")
                && !grid.contains("Loading")
        });
        source.send_stdin(&keys::ENTER);
        let expected = report.unwrap_or("null");
        source.wait_until(
            "prepared attachment reports switching client timezone",
            |grid| {
                grid.contains("DEST TIMEZONES")
                    && grid.contains("Asia/Kolkata")
                    && grid.contains(expected)
            },
        );
        // An ordinary fake send override cannot intercept this prepared AttachClient.
        // Observing the report here verifies capture before the direct preparation socket write.
        yinz_session::sessions::kill_session(&source.session_name().into()).unwrap();
        source.quit();
        destination.wait_for_exit();
    }
}

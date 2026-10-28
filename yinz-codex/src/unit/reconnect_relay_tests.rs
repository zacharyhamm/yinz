use super::*;

fn read_reports(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn reconnect_relays_terminal_events_and_new_thread_without_reselection() {
    for (review, status, event) in [
        (false, "failed", "StopFailure"),
        (true, "completed", "Stop"),
        (true, "interrupted", "Interrupt"),
        (true, "failed", "StopFailure"),
        (false, "new", "SessionEnd"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let reports = root.path().join("reports");
        let reporter = root.path().join("reporter");
        yinz_test_support::write_warm_script(
            &reporter,
            "cat >> \"$YINZ_TEST_REPORT_OUTPUT\"\nprintf '\\n' >> \"$YINZ_TEST_REPORT_OUTPUT\"",
        );
        let mut registration = registration(root.path(), "aaaaaaaaaaaa");
        registration.registry.env.insert(
            "YINZ_TEST_REPORT_OUTPUT".into(),
            reports.to_str().unwrap().into(),
        );
        registration.reporter = Some(reporter);
        for replacement in [false, true] {
            let (ui_stream, relay_stream) = tokio::net::UnixStream::pair().unwrap();
            let (backend_stream, server_stream) = tokio::io::duplex(16384);
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
            let scenario = async {
                let (client, server) = tokio::join!(
                    tokio_tungstenite::client_async("ws://localhost", ui_stream),
                    tokio_tungstenite::accept_async(server_stream)
                );
                let (mut ui, _) = client.unwrap();
                let mut backend = server.unwrap();
                let value = if !replacement {
                    let request = Message::text(json!({"id":7,"method":if review {"review/start"} else {"turn/start"},"params":{"threadId":"selected"}}).to_string());
                    ui.send(request.clone()).await.unwrap();
                    assert_eq!(backend.next().await.unwrap().unwrap(), request);
                    if review {
                        json!({"id":7,"result":{"reviewThreadId":"selected","turn":{"id":"parent"}}})
                    } else {
                        json!({"method":"turn/started","params":{"threadId":"selected","turn":{"id":"parent"}}})
                    }
                } else if status == "new" {
                    let request = Message::text(r#"{"id":7,"method":"thread/start"}"#);
                    ui.send(request.clone()).await.unwrap();
                    assert_eq!(backend.next().await.unwrap().unwrap(), request);
                    json!({"id":7,"result":{"thread":{"id":"new"}}})
                } else {
                    json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"parent","status":status,"error":{"message":"failure details"}}}})
                };
                let message = Message::text(format!(" {value} "));
                backend.send(message.clone()).await.unwrap();
                assert_eq!(ui.next().await.unwrap().unwrap(), message);
                if replacement {
                    let recorded = read_reports(&reports);
                    assert_eq!(recorded[0]["hook_event_name"], event);
                    assert_eq!(recorded[0]["session_id"], "selected");
                    if status == "new" {
                        assert_eq!(recorded.len(), 2);
                        assert_eq!(recorded[0]["reason"], "clear");
                        assert_eq!(recorded[1]["hook_event_name"], "SessionStart");
                        assert_eq!(recorded[1]["session_id"], "new");
                        assert_eq!(recorded[1]["source"], "clear");
                    } else {
                        assert_eq!(recorded.len(), 1);
                        assert_eq!(recorded[0]["turn_id"], "parent");
                        backend.send(message.clone()).await.unwrap();
                        assert_eq!(ui.next().await.unwrap().unwrap(), message);
                        assert_eq!(read_reports(&reports), recorded);
                    }
                } else {
                    assert!(!reports.exists());
                }
                ui.close(None).await.unwrap();
                assert!(matches!(
                    backend.next().await.unwrap().unwrap(),
                    Message::Close(_)
                ));
                assert!(matches!(
                    ui.next().await.unwrap().unwrap(),
                    Message::Close(_)
                ));
            };
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                tokio::join!(serve, scenario);
            })
            .await
            .unwrap();
        }
    }
}

#[tokio::test]
async fn reconnect_replays_cancelled_selection_reports_before_new_requests() {
    let root = tempfile::tempdir().unwrap();
    let entered = root.path().join("entered");
    assert!(Command::new("mkfifo")
        .arg(&entered)
        .status()
        .unwrap()
        .success());
    let pid_file = root.path().join("pid");
    let reports = root.path().join("reports");
    let stalled = root.path().join("stalled");
    let reporter = root.path().join("reporter");
    yinz_test_support::write_warm_script(
        &stalled,
        "printf '%s' \"$$\" > \"$YINZ_TEST_REPORT_PID\"\ncat >/dev/null\nprintf x > \"$YINZ_TEST_REPORT_ENTERED\"\nexec sleep 60",
    );
    yinz_test_support::write_warm_script(
        &reporter,
        "cat >> \"$YINZ_TEST_REPORT_OUTPUT\"\nprintf '\\n' >> \"$YINZ_TEST_REPORT_OUTPUT\"",
    );
    let mut registration = registration(root.path(), "aaaaaaaaaaaa");
    registration.registry.env.extend([
        (
            "YINZ_TEST_REPORT_PID".into(),
            pid_file.to_str().unwrap().into(),
        ),
        (
            "YINZ_TEST_REPORT_ENTERED".into(),
            entered.to_str().unwrap().into(),
        ),
        (
            "YINZ_TEST_REPORT_OUTPUT".into(),
            reports.to_str().unwrap().into(),
        ),
    ]);
    registration.reporter = Some(stalled);
    let (ui_stream, relay_stream) = tokio::net::UnixStream::pair().unwrap();
    let (backend_stream, server_stream) = tokio::io::duplex(16384);
    let mut serve = Box::pin(async {
        let ui = tokio_tungstenite::accept_async(relay_stream).await.unwrap();
        relay(
            ui,
            backend_stream,
            &registration,
            Path::new("/usr/bin/true"),
        )
        .await
    });
    let scenario = async {
        let (client, server) = tokio::join!(
            tokio_tungstenite::client_async("ws://localhost", ui_stream),
            tokio_tungstenite::accept_async(server_stream)
        );
        let (mut ui, _) = client.unwrap();
        let mut backend = server.unwrap();
        let request =
            Message::text(r#"{"id":1,"method":"turn/start","params":{"threadId":"selected"}}"#);
        ui.send(request.clone()).await.unwrap();
        assert_eq!(backend.next().await.unwrap().unwrap(), request);
        let request = Message::text(r#"{"id":2,"method":"thread/start"}"#);
        ui.send(request.clone()).await.unwrap();
        assert_eq!(backend.next().await.unwrap().unwrap(), request);
        backend
            .send(Message::text(
                r#"{"id":2,"result":{"thread":{"id":"new"}}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&entered).await.unwrap(), b"x");
        (ui, backend)
    };
    let (ui, backend) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        tokio::select! {
            result = &mut serve => panic!("relay ended before report cancellation: {result:?}"),
            peers = scenario => peers,
        }
    })
    .await
    .unwrap();
    // Transport replacement cancels the old relay while its first lifecycle report is pending.
    drop(serve);
    drop((ui, backend));
    let pid = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
    assert_bridge_child_stopped(pid).await;
    assert_eq!(registration.reports.borrow().len(), 2);
    registration.reporter = Some(reporter);
    let (ui_stream, relay_stream) = tokio::net::UnixStream::pair().unwrap();
    let (backend_stream, server_stream) = tokio::io::duplex(16384);
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
    let scenario = async {
        let (client, server) = tokio::join!(
            tokio_tungstenite::client_async("ws://localhost", ui_stream),
            tokio_tungstenite::accept_async(server_stream)
        );
        let (mut ui, _) = client.unwrap();
        let mut backend = server.unwrap();
        let request =
            Message::text(r#"{"id":3,"method":"turn/start","params":{"threadId":"new"}}"#);
        ui.send(request.clone()).await.unwrap();
        assert_eq!(backend.next().await.unwrap().unwrap(), request);
        let recorded = read_reports(&reports);
        assert_eq!(
            recorded.len(),
            2,
            "reports must precede new application work"
        );
        assert_eq!(recorded[0]["hook_event_name"], "SessionEnd");
        assert_eq!(recorded[0]["session_id"], "selected");
        assert_eq!(recorded[1]["hook_event_name"], "SessionStart");
        assert_eq!(recorded[1]["session_id"], "new");
        assert_eq!(recorded[1]["source"], "clear");
        ui.close(None).await.unwrap();
        assert!(matches!(
            backend.next().await.unwrap().unwrap(),
            Message::Close(_)
        ));
        assert!(matches!(
            ui.next().await.unwrap().unwrap(),
            Message::Close(_)
        ));
    };
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        tokio::join!(serve, scenario);
    })
    .await
    .unwrap();
    assert!(registration.reports.borrow().is_empty());
}

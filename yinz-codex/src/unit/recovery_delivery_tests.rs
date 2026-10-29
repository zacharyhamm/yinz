use super::*;
use tokio::io::AsyncReadExt;

async fn recovery_daemon(peers: &tokio::net::UnixListener, history: &str, review: bool) -> u32 {
    let (mut peer, _) = peers.accept().await.unwrap();
    let pid = peer.read_u32().await.unwrap();
    let mut ws = tokio_tungstenite::accept_async(peer).await.unwrap();
    for id in 0..=2 {
        if id == 1 {
            assert_eq!(
                ws.next().await.unwrap().unwrap(),
                Message::text(r#"{"method":"initialized"}"#)
            );
        }
        let request: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(request["id"], id);
        assert_eq!(
            request["method"],
            ["initialize", "thread/turns/list", "thread/read"][id]
        );
        let result = match id {
            0 => json!({}),
            1 if history == "unavailable" => {
                ws.send(Message::text(
                    json!({"id":1,"error":{"message":"history unavailable"}}).to_string(),
                ))
                .await
                .unwrap();
                return pid;
            },
            1 if review && history == "current" => json!({"data":[
                {"id":"review-child","rootTurnId":"known-turn","status":"completed"},
                {"id":"known-turn","status":"completed"}
            ]}),
            1 => json!({"data":[{
                "id":if history == "newer" {"newer-turn"} else {"known-turn"},
                "status":if review {"completed"} else {"failed"},
                "error":{"message":"gap failure"}
            }]}),
            _ => json!({"thread":{
                "id":"selected",
                "status":{"type":if history == "active" {"active"} else {"idle"}}
            }}),
        };
        ws.send(Message::text(json!({"id":id,"result":result}).to_string()))
            .await
            .unwrap();
        if id == 1 && history == "newer" {
            break;
        }
    }
    pid
}

#[tokio::test]
async fn reconnect_recovery_delivers_only_proven_current_outcomes() {
    const TEST: &str =
        "tests::recovery_delivery_tests::reconnect_recovery_delivers_only_proven_current_outcomes";
    if let Ok(socket) = std::env::var("YINZ_TEST_RECOVERY_PEER") {
        let mut peer = tokio::net::UnixStream::connect(socket).await.unwrap();
        peer.write_u32(std::process::id()).await.unwrap();
        let mut stream = tokio::io::join(tokio::io::stdin(), tokio::io::stderr());
        let _ = tokio::io::copy_bidirectional(&mut peer, &mut stream).await;
        // Let the owner's cancellation, rather than socket EOF, terminate this proxy.
        std::future::pending::<()>().await;
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let peer_socket = root.path().join("peer.sock");
    let peers = tokio::net::UnixListener::bind(&peer_socket).unwrap();
    let program = root.path().join("proxy");
    let reporter = root.path().join("reporter");
    yinz_test_support::write_warm_script(
        &program,
        &format!(
            "export YINZ_TEST_RECOVERY_PEER='{}'\nexec '{}' --exact '{TEST}' --nocapture 2>&1 >/dev/null",
            peer_socket.display(),
            std::env::current_exe().unwrap().display(),
        ),
    );
    yinz_test_support::write_warm_script(
        &reporter,
        "cat >> \"$YINZ_TEST_RECOVERY_REPORT\"\nprintf '\\n' >> \"$YINZ_TEST_RECOVERY_REPORT\"",
    );

    for (review, cancelled, history) in [
        (false, false, "current"),
        (true, false, "current"),
        (false, true, "current"),
        (false, true, "newer"),
        (false, true, "active"),
        (false, true, "unavailable"),
        (false, false, "unavailable"),
        (true, false, "newer"),
        (true, true, "current"),
        (true, true, "newer"),
        (true, true, "active"),
        (true, true, "unavailable"),
    ] {
        let case = tempfile::tempdir_in(root.path()).unwrap();
        let output = case.path().join("reports.jsonl");
        let mut registration = Registration::new(
            case.path(),
            HashMap::from([(
                "YINZ_TEST_RECOVERY_REPORT".into(),
                output.to_str().unwrap().into(),
            )]),
        )
        .unwrap();
        registration.reporter = Some(reporter.clone());
        {
            let mut conversations = registration.conversations.borrow_mut();
            conversations.select("selected", &registration).unwrap();
            if !cancelled || review {
                if review {
                    conversations.review = Some(Review {
                        request_id: None,
                        thread: "selected".into(),
                        turn: Some("known-turn".into()),
                        early_terminal: None,
                    });
                } else {
                    conversations.current_turn = Some("known-turn".into());
                }
            }
        }
        let mut expected = json!({
            "hook_event_name":if review {"Stop"} else {"StopFailure"},
            "session_id":"selected", "turn_id":"known-turn"
        });
        if !review {
            expected["last_assistant_message"] = if cancelled {
                "cancelled delivery"
            } else {
                "gap failure"
            }
            .into();
        }
        if cancelled {
            if review {
                let hooks = registration.conversations.borrow_mut().response(
                    &json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"known-turn","status":"completed"}}}),
                    &registration,
                ).unwrap();
                assert_eq!(hooks, vec![expected.clone()]);
                assert!(registration.conversations.borrow().review_turns.is_empty());
            }
            registration
                .reports
                .borrow_mut()
                .push_back(expected.clone());
        }
        let (result, pid) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            tokio::join!(
                registration.reconnect(&program),
                recovery_daemon(&peers, history, review),
            )
        })
        .await
        .expect("reconnect recovery stalled");
        result.unwrap();
        assert_bridge_child_stopped(pid).await;
        assert!(registration.reports.borrow().is_empty());
        assert!(registration
            .conversations
            .borrow()
            .reported_review
            .is_none());
        if history == "current" {
            let reports: Vec<Value> = std::fs::read_to_string(&output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(reports, vec![expected]);
        } else {
            assert!(!output.exists(), "unsafe report: {history}, {cancelled}");
        }
        assert_eq!(
            registration.conversations.borrow().outstanding_turn(),
            (!cancelled && history != "current").then_some(("selected", "known-turn")),
            "{history}, review={review}, cancelled={cancelled}"
        );
    }
}

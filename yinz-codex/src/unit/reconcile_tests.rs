use super::*;

async fn recovered_turn(review: bool, replies: Vec<Value>) -> Result<Option<Value>> {
    let (client, server) = tokio::io::duplex(16384);
    let query = async {
        let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost", client)
            .await
            .unwrap();
        read_recovered_turn(&mut ws, "selected", "known-turn", review).await
    };
    let daemon = async {
        let mut ws = tokio_tungstenite::accept_async(server).await.unwrap();
        let mut previous = Value::Null;
        for (id, reply) in replies.into_iter().enumerate() {
            if id == 1 {
                assert_eq!(
                    ws.next().await.unwrap().unwrap(),
                    Message::text(r#"{"method":"initialized"}"#)
                );
            }
            let request: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["id"], id);
            let found = previous["result"]["data"]
                .as_array()
                .is_some_and(|turns| turns.iter().any(|turn| turn["id"] == "known-turn"));
            let method = if id == 0 {
                "initialize"
            } else if found {
                "thread/read"
            } else {
                "thread/turns/list"
            };
            assert_eq!(request["method"], method);
            if method == "thread/turns/list" {
                let mut params = json!({
                    "threadId":"selected", "limit":if review {50} else {1},
                    "sortDirection":"desc", "itemsView":"notLoaded"
                });
                if id > 1 {
                    params["cursor"] = previous["result"]["nextCursor"].clone();
                }
                assert_eq!(request["params"], params);
            } else if method == "thread/read" {
                assert_eq!(request["params"], json!({"threadId":"selected"}));
            }
            ws.send(Message::text(reply.to_string())).await.unwrap();
            previous = reply;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(query, daemon).0
    })
    .await
    .expect("recovery query stalled")
}

#[tokio::test]
async fn reconnect_reconciliation_recovers_only_the_latest_terminal_turn_of_an_idle_thread() {
    for status in ["completed", "interrupted", "failed"] {
        let turn = json!({
            "id":"known-turn", "status":status,
            "error":{"message":"preserved failure detail"}, "items":[]
        });
        let result = recovered_turn(
            false,
            vec![
                json!({"id":0,"result":{}}),
                json!({"id":1,"result":{"data":[turn.clone()],"nextCursor":"older"}}),
                json!({"id":2,"result":{"thread":{"id":"selected","status":{"type":"idle"}}}}),
            ],
        )
        .await
        .unwrap();
        assert_eq!(result, Some(turn), "{status}");
    }
}

#[tokio::test]
async fn reconnect_reconciliation_does_not_end_newer_active_or_unknown_work() {
    for data in [
        json!([]),
        json!([{"id":"newer-turn","status":"completed"}]),
        json!([{"id":"newer-turn","status":"inProgress"}]),
        json!([{"id":"known-turn","status":"inProgress"}]),
        json!([{"id":"known-turn","status":"unknown"}]),
    ] {
        assert_eq!(
            recovered_turn(
                false,
                vec![
                    json!({"id":0,"result":{}}),
                    json!({"id":1,"result":{"data":data}}),
                ]
            )
            .await
            .unwrap(),
            None,
            "{data}"
        );
    }
    for status in ["active", "notLoaded", "systemError", "unknown"] {
        assert_eq!(
            recovered_turn(
                false,
                vec![
                    json!({"id":0,"result":{}}),
                    json!({"id":1,"result":{"data":[{"id":"known-turn","status":"failed"}]}}),
                    json!({"id":2,"result":{"thread":{"id":"selected","status":{"type":status}}}}),
                ]
            )
            .await
            .unwrap(),
            None,
            "{status}"
        );
    }
}

#[tokio::test]
async fn reconnect_reconciliation_rejects_rpc_errors_malformed_data_and_wrong_identity() {
    for reply in [
        json!({"id":0,"error":{"message":"unsupported"}}),
        json!({"id":0,"result":false}),
    ] {
        assert!(
            recovered_turn(false, vec![reply.clone()]).await.is_err(),
            "{reply}"
        );
    }
    for reply in [
        json!({"id":1,"error":{"message":"unsupported"}}),
        json!({"id":1,"result":{}}),
        json!({"id":1,"result":{"data":{}}}),
        json!({"id":1,"result":{"data":[null]}}),
        json!({"id":1,"result":{"data":[{"id":"known-turn"}]}}),
    ] {
        assert!(
            recovered_turn(false, vec![json!({"id":0,"result":{}}), reply.clone()])
                .await
                .is_err(),
            "{reply}"
        );
    }
    for reply in [
        json!({"id":2,"error":{"message":"thread unavailable"}}),
        json!({"id":2,"result":{}}),
        json!({"id":2,"result":{"thread":false}}),
        json!({"id":2,"result":{"thread":{"id":"other","status":{"type":"idle"}}}}),
        json!({"id":2,"result":{"thread":{"id":"selected"}}}),
        json!({"id":2,"result":{"thread":{"id":"selected","status":{"type":false}}}}),
    ] {
        assert!(
            recovered_turn(
                false,
                vec![
                    json!({"id":0,"result":{}}),
                    json!({"id":1,"result":{"data":[{"id":"known-turn","status":"failed"}]}}),
                    reply.clone(),
                ]
            )
            .await
            .is_err(),
            "{reply}"
        );
    }
}

#[tokio::test]
async fn reconnect_reconciliation_finds_a_review_parent_after_its_completed_children() {
    let parent = json!({"id":"known-turn","status":"completed"});
    let child = json!({
        "id":"child", "rootTurnId":"known-turn", "status":"completed"
    });
    for pages in [
        vec![json!({"data":[child.clone(), parent.clone()]})],
        vec![
            json!({"data":[child.clone()],"nextCursor":"older"}),
            json!({"data":[parent.clone()]}),
        ],
    ] {
        let mut replies = vec![json!({"id":0,"result":{}})];
        for (index, page) in pages.into_iter().enumerate() {
            replies.push(json!({"id":index+1,"result":page}));
        }
        replies.push(json!({"id":replies.len(),"result":{
            "thread":{"id":"selected","status":{"type":"idle"}}
        }}));
        assert_eq!(
            recovered_turn(true, replies).await.unwrap(),
            Some(parent.clone())
        );
    }
}

#[tokio::test]
async fn reconnect_reconciliation_requires_proof_that_newer_turns_are_finished_review_children() {
    for child in [
        json!({"id":"child","rootTurnId":"unrelated","status":"completed"}),
        json!({"id":"child","rootTurnId":"known-turn","status":"inProgress"}),
        json!({"id":"child","status":"completed"}),
        json!({"id":"child","rootTurnId":null,"status":"completed"}),
    ] {
        assert_eq!(
            recovered_turn(
                true,
                vec![
                    json!({"id":0,"result":{}}),
                    json!({"id":1,"result":{"data":[
                    child.clone(), {"id":"known-turn","status":"completed"}
                ],"nextCursor":"older"}}),
                ]
            )
            .await
            .unwrap(),
            None,
            "{child}"
        );
    }
}

#[tokio::test]
async fn reconnect_reconciliation_rejects_malformed_review_history_and_repeated_cursors() {
    for child in [
        json!({"rootTurnId":"known-turn","status":"completed"}),
        json!({"id":"child","rootTurnId":"known-turn"}),
    ] {
        assert!(
            recovered_turn(
                true,
                vec![
                    json!({"id":0,"result":{}}),
                    json!({"id":1,"result":{"data":[child.clone()]}}),
                ]
            )
            .await
            .is_err(),
            "{child}"
        );
    }
    for cursor in [json!(false), json!("")] {
        assert!(
            recovered_turn(
                true,
                vec![
                    json!({"id":0,"result":{}}),
                    json!({"id":1,"result":{"data":[],"nextCursor":cursor.clone()}}),
                ]
            )
            .await
            .is_err(),
            "{cursor}"
        );
    }
    assert!(recovered_turn(
        true,
        vec![
            json!({"id":0,"result":{}}),
            json!({"id":1,"result":{"data":[],"nextCursor":"same"}}),
            json!({"id":2,"result":{"data":[],"nextCursor":"same"}}),
        ]
    )
    .await
    .is_err());
}

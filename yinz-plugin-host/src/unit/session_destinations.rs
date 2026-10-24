use super::*;
use yinz_types::data::{SessionInfo, SessionListSnapshot};

#[test]
fn inventory_deduplicates_only_the_current_machine_and_keeps_saved_sessions_on_it() {
    for current_machine in [Destination::Local, Destination::Remote("host-a".into())] {
        let entry = |destination| Entry {
            destination,
            name: Some("same".into()),
            status: "Running".into(),
            current: false,
            saved: false,
        };
        let current = SessionInfo {
            name: "same".into(),
            is_current_session: true,
            ..Default::default()
        };
        let inventory = merge(
            SessionListSnapshot {
                live_sessions: vec![current],
                resurrectable_sessions: vec![
                    ("same".into(), Default::default()),
                    ("saved".into(), Default::default()),
                ],
            },
            Inventory {
                current_machine: current_machine.clone(),
                entries: vec![
                    entry(Destination::Local),
                    entry(Destination::Remote("host-a".into())),
                    entry(Destination::Remote("host-b".into())),
                ],
                current_server_only: false,
            },
        );
        assert_eq!(inventory.entries.len(), 4);
        assert_eq!(
            inventory
                .entries
                .iter()
                .filter(|entry| entry.current)
                .count(),
            1
        );
        assert!(inventory
            .entries
            .iter()
            .all(|entry| entry.destination != current_machine));
        let saved = inventory
            .entries
            .iter()
            .filter(|entry| entry.saved)
            .collect::<Vec<_>>();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].destination, Destination::CurrentServer);
    }
}

#[test]
fn requests_use_the_acting_client_and_unique_reply_tokens() {
    use yinz_runtime::channels::{self, SenderWithContext};
    let (server, requests) = channels::unbounded();
    let worker = std::thread::spawn(move || {
        let mut tokens = std::collections::HashSet::new();
        for expected in [2, 3] {
            let (instruction, _) = requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let ServerInstruction::SessionRequest {
                client,
                request,
                reply,
            } = instruction
            else {
                panic!("expected client-scoped session request");
            };
            assert_eq!(client, crate::ClientId::new(expected));
            assert!(tokens.insert(request.token));
            reply
                .send(Ok(ClientReply::Inventory(Inventory {
                    current_machine: Destination::Remote(format!("host-{expected}")),
                    entries: vec![],
                    current_server_only: false,
                })))
                .unwrap();
        }
    });
    let mut env = crate::test_env::test_env();
    env.senders.to_server = Some(SenderWithContext::new(server));
    for client in [2, 3] {
        env.client_id = crate::ClientId::new(client);
        assert_ne!(env.client_id, env.owner_client_id);
        let id = scoped_id(&env, "picker").unwrap();
        let reply = request(&mut env, ClientRequest::Inventory(id)).unwrap();
        assert_eq!(
            reply,
            ClientReply::Inventory(Inventory {
                current_machine: Destination::Remote(format!("host-{client}")),
                entries: vec![],
                current_server_only: false,
            })
        );
    }
    worker.join().unwrap();
    assert!(scoped_id(&env, "").is_err());
    assert!(scoped_id(&env, &"x".repeat(129)).is_err());
}

#[test]
fn preparation_ownership_requires_a_confirmed_rejection_or_completion() {
    use yinz_runtime::channels::{self, SenderWithContext};
    let (server, requests) =
        channels::unbounded::<(ServerInstruction, yinz_types::diagnostics::ErrorContext)>();
    let worker = std::thread::spawn(move || {
        for result in [
            Ok(ClientReply::Pending),
            Err("already pending".into()),
            Err("already pending".into()),
            Err("cancel failed".into()),
            Ok(ClientReply::Pending),
            Ok(ClientReply::Complete),
            Ok(ClientReply::Pending),
            Ok(ClientReply::Complete),
        ] {
            let ServerInstruction::SessionRequest { reply, .. } = requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .0
            else {
                panic!("expected session request")
            };
            reply.send(result).unwrap();
        }
    });
    let mut env = crate::test_env::test_env();
    env.senders.to_server = Some(SenderWithContext::new(server));
    let prepare = |id: &str| SwitchRequest::Prepare {
        id: id.into(),
        target: SwitchTarget {
            destination: Destination::Local,
            name: "target".into(),
            saved: false,
        },
    };
    for (operation, succeeds, owned) in [
        (prepare("pending"), true, true),
        (prepare("pending"), false, true),
        (prepare("rejected"), false, true),
        (SwitchRequest::Cancel("pending".into()), false, true),
        (SwitchRequest::Commit("pending".into()), true, true),
        (SwitchRequest::Cancel("pending".into()), true, false),
        (prepare("pending"), true, true),
        (SwitchRequest::Commit("pending".into()), true, false),
    ] {
        assert_eq!(
            request(&mut env, ClientRequest::Switch(operation)).is_ok(),
            succeeds
        );
        let expected = if owned {
            [(env.client_id, "pending".into())].into()
        } else {
            Default::default()
        };
        assert_eq!(env.session_preparations, expected);
    }
    worker.join().unwrap();
}

#[test]
fn timed_out_preparation_is_still_cancelled_on_its_original_client() {
    use yinz_runtime::channels::{self, SenderWithContext};
    let (server, requests) =
        channels::unbounded::<(ServerInstruction, yinz_types::diagnostics::ErrorContext)>();
    let worker = std::thread::spawn(move || {
        let ServerInstruction::SessionRequest {
            client,
            request,
            reply,
        } = requests
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .0
        else {
            panic!("expected preparation")
        };
        let ClientRequest::Switch(SwitchRequest::Prepare { id, .. }) = request.request else {
            panic!("expected preparation")
        };
        let ServerInstruction::SessionRequest {
            client: cancelled_client,
            request,
            ..
        } = requests
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .0
        else {
            panic!("expected cancellation")
        };
        assert_eq!(cancelled_client, client);
        assert_eq!(
            request.request,
            ClientRequest::Switch(SwitchRequest::Cancel(id))
        );
        assert!(reply.send(Ok(ClientReply::Pending)).is_err());
    });
    let mut env = crate::test_env::test_env();
    env.generation = Some(42);
    env.client_id = ClientId::new(2);
    env.senders.to_server = Some(SenderWithContext::new(server));
    let id = scoped_id(&env, "pending").unwrap();
    let result = request(
        &mut env,
        ClientRequest::Switch(SwitchRequest::Prepare {
            id,
            target: SwitchTarget {
                destination: Destination::Local,
                name: "target".into(),
                saved: false,
            },
        }),
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("client did not answer"));
    assert_eq!(env.session_preparations.len(), 1);
    env.client_id = ClientId::new(3);
    cancel_preparations(&mut env);
    assert!(env.session_preparations.is_empty());
    worker.join().unwrap();
}

#[test]
fn unsupported_client_falls_back_to_current_server_sessions() {
    use yinz_runtime::channels::{self, SenderWithContext};
    let (server, requests) =
        channels::unbounded::<(ServerInstruction, yinz_types::diagnostics::ErrorContext)>();
    let worker = std::thread::spawn(move || {
        for _ in 0..3 {
            let ServerInstruction::SessionRequest { reply, .. } = requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .0
            else {
                panic!("expected session request")
            };
            reply.send(Err(UNSUPPORTED_CLIENT.into())).unwrap();
        }
    });
    let mut env = crate::test_env::test_env();
    env.senders.to_server = Some(SenderWithContext::new(server));
    let id = scoped_id(&env, "picker").unwrap();
    assert_eq!(
        client_inventory(&mut env, id).unwrap(),
        Inventory {
            current_machine: Destination::CurrentServer,
            entries: vec![],
            current_server_only: true,
        }
    );
    let response = |env: &mut crate::plugin_map::PluginEnv| {
        let bytes = env.response_sink.take().unwrap();
        let json =
            yinz_plugin_api::generated_api::api::plugin_command::SessionDestinationResponse::decode(
                bytes.as_slice(),
            )
            .unwrap()
            .json;
        serde_json::from_str::<std::result::Result<serde_json::Value, String>>(&json).unwrap()
    };
    // There is nothing to cancel on a client that cannot prepare a switch.
    switch(&mut env, SwitchRequest::Cancel("picker".into())).unwrap();
    assert_eq!(response(&mut env), Ok(serde_json::json!("Complete")));
    switch(
        &mut env,
        SwitchRequest::Prepare {
            id: "picker".into(),
            target: SwitchTarget {
                destination: Destination::CurrentServer,
                name: "target".into(),
                saved: false,
            },
        },
    )
    .unwrap();
    assert_eq!(response(&mut env), Err(UNSUPPORTED_CLIENT.to_owned()));
    assert!(env.session_preparations.is_empty());
    worker.join().unwrap();
}

use super::capturing_os::CapturingOs;
use super::*;
use yinz_types::session_destination::{ClientRequest, Request, UNSUPPORTED_CLIENT};

fn request(token: &str) -> Request {
    Request {
        token: token.into(),
        request: ClientRequest::Inventory("picker".into()),
    }
}

#[test]
fn session_requests_reach_only_attached_clients_that_advertise_them() {
    let state = Arc::new(RwLock::new(SessionState::new()));
    let (older, current, detached) = {
        let mut state = state.write().unwrap();
        let older = state.new_client().unwrap();
        let current = state.new_client().unwrap();
        let detached = state.new_client().unwrap();
        state.attach_interactive(older, false);
        state.attach_interactive(current, true);
        (older, current, detached)
    };
    let data = Arc::new(RwLock::new(None));
    let capture = CapturingOs::default();
    let mut os: Box<dyn ServerOsApi> = Box::new(capture.clone());

    for (client, refusal) in [
        (older, UNSUPPORTED_CLIENT),
        (detached, "The requesting client is no longer attached"),
    ] {
        let (reply, replies) = std::sync::mpsc::channel();
        route_session_request(client, request("refused"), reply, &mut os, &state, &data);
        // Answered before returning: the plugin never waits for a reply that cannot come.
        assert_eq!(replies.try_recv().unwrap(), Err(refusal.to_owned()));
    }
    assert!(state.read().unwrap().session_requests.is_empty());

    let (reply, replies) = std::sync::mpsc::channel();
    route_session_request(current, request("forwarded"), reply, &mut os, &state, &data);
    assert!(replies.try_recv().is_err());
    assert!(state
        .read()
        .unwrap()
        .session_requests
        .contains_key(&(current, "forwarded".to_owned())));
    let sent = capture.server_to_client_messages.lock().clone();
    assert_eq!(
        sent,
        HashMap::from([(
            current,
            vec![ServerToClientMsg::SessionRequest(request("forwarded"))]
        )])
    );
}

#[test]
fn leaving_or_watching_forgets_session_request_support() {
    let mut state = SessionState::new();
    let client = state.new_client().unwrap();
    state.attach_interactive(client, true);
    assert!(state.supports_session_requests(&client));
    state.convert_client_to_watcher(client);
    assert!(!state.supports_session_requests(&client));

    let client = state.new_client().unwrap();
    state.attach_interactive(client, true);
    state.remove_client(client);
    assert!(!state.supports_session_requests(&client));
}

#[test]
fn attachment_generation_pending_requests_finish_on_owner_loss_and_refusal() {
    let state = Arc::new(RwLock::new(SessionState::new()));
    let data = Arc::new(RwLock::new(None));
    let attach = |number| {
        let mut state = state.write().unwrap();
        state.next_client_id = Some(number);
        let id = state.new_client().unwrap();
        state.attach_interactive(id, true);
        id
    };
    let mut os: Box<dyn ServerOsApi> = Box::new(CapturingOs::default());
    let route = |owner, os: &mut Box<dyn ServerOsApi>| {
        let (sender, receiver) = std::sync::mpsc::channel();
        route_session_request(owner, request("same-token"), sender, os, &state, &data);
        receiver
    };
    let old = attach(1);
    let old_reply = route(old, &mut os);
    let new = attach(1);
    assert!(old_reply.try_recv().unwrap().is_err());
    let new_reply = route(new, &mut os);
    state.write().unwrap().remove_client(old);
    assert!(matches!(
        new_reply.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    state.write().unwrap().remove_client(new);
    assert!(new_reply.try_recv().unwrap().is_err());

    let sealed = attach(2);
    let capture = CapturingOs::default();
    *capture.refused.lock() = Some((sealed, 1, std::time::Instant::now()));
    os = Box::new(capture);
    assert!(route(sealed, &mut os).try_recv().unwrap().is_err());
    assert!(state.read().unwrap().session_requests.is_empty());
}

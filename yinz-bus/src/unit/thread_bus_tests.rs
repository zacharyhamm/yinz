use crate::{screen::ScreenInstruction, thread_bus::ThreadSenders, ClientId};
use yinz_runtime::channels::{unbounded, Shutdown};

#[test]
fn save_session_preserves_delivery_errors_during_shutdown() {
    let shutdown = Shutdown::default();
    let (sender, receiver) = unbounded();
    let senders = ThreadSenders {
        to_screen: Some(shutdown.sender(sender)),
        ..Default::default()
    };
    let client_id = ClientId::new(1);
    shutdown.begin();

    senders
        .send_to_screen(ScreenInstruction::SaveSession(client_id, None))
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap().0,
        ScreenInstruction::SaveSession(id, None) if id == client_id
    ));

    drop(receiver);
    assert!(senders
        .send_to_screen(ScreenInstruction::SaveSession(client_id, None))
        .is_err());
    assert!(senders.send_to_screen(ScreenInstruction::Render).is_ok());
}

#[test]
fn replies_survive_queued_notifications_and_cleanup_without_replay() {
    use crate::plugins::{PluginId, PluginInstruction};
    use crate::thread_bus::PluginReplies;
    use yinz_types::data::Event;
    let (sender, receiver) = unbounded();
    let senders = ThreadSenders {
        to_plugin: Some(Shutdown::default().sender(sender)),
        ..Default::default()
    };
    let reply = |generation, client, value: &str| {
        PluginInstruction::PluginEvent(
            PluginId::new(1),
            ClientId::new(client),
            generation,
            Event::CustomMessage(value.into(), String::new()),
        )
    };
    let old = PluginReplies::new(123);
    let replacement = PluginReplies::new(124);
    let notification = || {
        let PluginInstruction::PluginReply(_, _, _, _, reply) = receiver.try_recv().unwrap().0
        else {
            panic!("expected retained reply")
        };
        reply
    };
    senders.send_to_plugin(reply(123, 2, "1")).unwrap();
    let consumed = notification();
    assert!(
        matches!(consumed.claim(), Some((client, Event::CustomMessage(value, _))) if client == ClientId::new(2) && value == "1")
    );
    assert!(consumed.claim().is_none());
    senders.send_to_plugin(reply(123, 3, "2")).unwrap();
    let queued = notification();
    senders.send_to_plugin(reply(124, 4, "3")).unwrap();
    let current = notification();
    let cleanup = old.begin_cleanup();
    senders.send_to_plugin(reply(123, 5, "4")).unwrap();
    assert!(receiver.try_recv().is_err());
    assert!(queued.claim().is_none());
    assert!(
        matches!(cleanup.try_recv().unwrap(), (client, Event::CustomMessage(value, _)) if client == ClientId::new(3) && value == "2")
    );
    assert!(
        matches!(cleanup.try_recv().unwrap(), (client, Event::CustomMessage(value, _)) if client == ClientId::new(5) && value == "4")
    );
    assert!(cleanup.try_recv().is_err());
    assert!(
        matches!(current.claim(), Some((client, Event::CustomMessage(value, _))) if client == ClientId::new(4) && value == "3")
    );
    drop(old);
    assert!(queued.claim().is_none());
    assert!(replacement.begin_cleanup().try_recv().is_err());
}

#[test]
fn disposal_discards_pending_replies() {
    use crate::plugins::{PluginId, PluginInstruction};
    use crate::thread_bus::PluginReplies;
    let (sender, receiver) = unbounded();
    let senders = ThreadSenders {
        to_plugin: Some(Shutdown::default().sender(sender)),
        ..Default::default()
    };
    let mailbox = PluginReplies::new(125);
    senders
        .send_to_plugin(PluginInstruction::PluginEvent(
            PluginId::new(1),
            ClientId::new(2),
            125,
            yinz_types::data::Event::Visible(true),
        ))
        .unwrap();
    let PluginInstruction::PluginReply(_, _, _, _, reply) = receiver.try_recv().unwrap().0 else {
        panic!("expected retained reply")
    };
    drop(mailbox);
    assert!(reply.claim().is_none());
}

#[test]
fn shutdown_keeps_background_jobs_until_plugins_finish() {
    use crate::background_jobs::BackgroundJob;
    let (sender, receiver) = unbounded();
    let senders = ThreadSenders {
        to_background_jobs: Some(Shutdown::default().sender(sender)),
        should_silently_fail: true,
        ..Default::default()
    };
    senders.exit_threads(|| {}, || assert!(receiver.try_recv().is_err()));
    assert!(matches!(
        receiver.try_recv().unwrap().0,
        BackgroundJob::Exit
    ));
}

#[test]
fn detached_senders_do_not_retain_session_ownership() {
    use std::sync::Arc;
    use yinz_ipc::session_ownership::SessionOwnership;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("session");
    let ownership = Arc::new(SessionOwnership::acquire(&socket).unwrap().unwrap());
    let senders = ThreadSenders {
        session_ownership: Some(Arc::downgrade(&ownership)),
        ..Default::default()
    };
    let detached = senders.clone();
    assert!(SessionOwnership::acquire(&socket).unwrap().is_none());
    ownership.shutdown().unwrap();
    drop(ownership);
    assert!(SessionOwnership::acquire(&socket).unwrap().is_some());
    assert!(detached.session_ownership.unwrap().upgrade().is_none());
}

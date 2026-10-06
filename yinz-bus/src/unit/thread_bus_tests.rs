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
fn unload_replies_bypass_the_plugin_bus_only_for_the_retiring_generation() {
    use crate::plugins::{PluginId, PluginInstruction};
    use crate::thread_bus::UnloadReplies;
    use yinz_types::data::Event;
    let (sender, receiver) = unbounded();
    let senders = ThreadSenders {
        to_plugin: Some(Shutdown::default().sender(sender)),
        ..Default::default()
    };
    let reply = |generation| {
        PluginInstruction::DenoEvent(
            PluginId::new(1),
            ClientId::new(2),
            generation,
            Event::Visible(true),
        )
    };
    let cleanup = UnloadReplies::new(123);
    senders.send_to_plugin(reply(123)).unwrap();
    assert!(
        matches!(cleanup.receiver.try_recv().unwrap(), (client, Event::Visible(true)) if client == ClientId::new(2))
    );
    assert!(receiver.try_recv().is_err());
    senders.send_to_plugin(reply(124)).unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap().0,
        PluginInstruction::DenoEvent(_, _, 124, _)
    ));
    drop(cleanup);
    senders.send_to_plugin(reply(123)).unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap().0,
        PluginInstruction::DenoEvent(_, _, 123, _)
    ));
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

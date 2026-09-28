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

use super::*;
use std::sync::atomic::Ordering;

#[test]
fn retired_signal_listener_preserves_resize_and_quit_for_the_current_connection() {
    for resize in [true, false] {
        let (mut current, _handle) = FakeClientOsApi::new(crate::TERMINAL_SIZE, None);
        let (signal_tx, signal_rx) = crossbeam::channel::bounded(0);
        current.signal_rx = signal_rx;
        let retired = current.clone();
        let session_name = current.session_name.clone();
        let handoff = session_name.lock().unwrap();
        let old_listener = std::thread::spawn(move || {
            retired.handle_signals(
                Box::new(|| panic!("retired listener delivered a resize")),
                Box::new(|| panic!("retired listener delivered a quit")),
            );
        });
        // Rendezvous proves the old listener consumed the event before handoff finishes.
        assert!(signal_tx
            .send_timeout(
                if resize {
                    SignalEvent::Resize
                } else {
                    SignalEvent::Quit
                },
                crate::default_timeout(),
            )
            .is_ok());
        current.generation = current.connection_generation.fetch_add(1, Ordering::AcqRel) + 1;
        drop(handoff);
        old_listener.join().unwrap();

        let (delivered, events) = std::sync::mpsc::channel();
        let new_listener = std::thread::spawn(move || {
            let resized = delivered.clone();
            current.handle_signals(
                Box::new(move || resized.send("resize").unwrap()),
                Box::new(move || delivered.send("quit").unwrap()),
            );
        });
        if resize {
            assert!(signal_tx
                .send_timeout(SignalEvent::Quit, crate::default_timeout())
                .is_ok());
        }
        new_listener.join().unwrap();
        assert_eq!(
            events.try_iter().collect::<Vec<_>>(),
            if resize {
                vec!["resize", "quit"]
            } else {
                vec!["quit"]
            }
        );
    }
}

#[test]
fn signal_callback_finishes_before_connection_generation_changes() {
    let (mut current, handle) = FakeClientOsApi::new(crate::TERMINAL_SIZE, None);
    let listener = current.clone();
    let callback = current.clone();
    let (entered, entry) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let signals = std::thread::spawn(move || {
        listener.handle_signals(
            Box::new(|| panic!("unexpected resize")),
            Box::new(move || {
                entered.send(()).unwrap();
                released.recv().unwrap();
                assert!(callback.is_current_connection());
            }),
        );
    });
    handle.signal_tx.send(SignalEvent::Quit).unwrap();
    entry.recv_timeout(crate::default_timeout()).unwrap();
    assert!(matches!(
        current.session_name.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    ));
    let switched = std::thread::spawn(move || {
        current.update_session_name("destination".into());
        assert!(current.is_current_connection());
    });
    release.send(()).unwrap();
    signals.join().unwrap();
    switched.join().unwrap();
}

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
            retired
                .start_signal_listener(
                    Box::new(|| panic!("retired listener delivered a resize")),
                    Box::new(|| panic!("retired listener delivered a quit")),
                )
                .unwrap()
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
        old_listener.join().unwrap().stop().unwrap();

        let (delivered, events) = std::sync::mpsc::channel();
        let new_listener = std::thread::spawn(move || {
            let resized = delivered.clone();
            current
                .start_signal_listener(
                    Box::new(move || resized.send("resize").unwrap()),
                    Box::new(move || delivered.send("quit").unwrap()),
                )
                .unwrap()
        });
        if resize {
            assert!(signal_tx
                .send_timeout(SignalEvent::Quit, crate::default_timeout())
                .is_ok());
        }
        let expected_events = if resize { 2 } else { 1 };
        let received: Vec<_> = (0..expected_events)
            .map(|_| events.recv_timeout(crate::default_timeout()).unwrap())
            .collect();
        new_listener.join().unwrap().stop().unwrap();
        assert_eq!(
            received,
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
        listener
            .start_signal_listener(
                Box::new(|| panic!("unexpected resize")),
                Box::new(move || {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    assert!(callback.is_current_connection());
                }),
            )
            .unwrap()
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
    signals.join().unwrap().stop().unwrap();
    switched.join().unwrap();
}

#[test]
fn cancelling_an_idle_fake_listener_releases_callbacks() {
    let (os, _handle) = FakeClientOsApi::new(Size { rows: 24, cols: 80 }, None);
    let (callback, released) = std::sync::mpsc::channel::<()>();
    let listener = os
        .start_signal_listener(
            Box::new(move || {
                let _ = callback.send(());
            }),
            Box::new(|| panic!("unexpected quit signal")),
        )
        .unwrap();
    let (done, stopped) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        listener.stop().unwrap();
        done.send(()).unwrap();
    });
    stopped
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    worker.join().unwrap();
    assert_eq!(
        released.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    );
}

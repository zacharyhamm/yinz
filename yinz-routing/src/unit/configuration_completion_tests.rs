use super::*;
use crossbeam::channel::{unbounded, RecvTimeoutError};
use yinz_runtime::channels::SenderWithContext;

#[test]
fn reconfiguration_waits_for_server_completion_past_the_action_timeout() {
    for write_to_disk in [false, true] {
        let (server_tx, server_rx) = unbounded();
        let (result_tx, result_rx) = unbounded();
        let route = std::thread::spawn(move || {
            let result = route_action(
                Action::Reconfigure {
                    config: "mouse_mode false".into(),
                    write_to_disk,
                },
                ClientId::new(1),
                None,
                None,
                ThreadSenders {
                    to_server: Some(SenderWithContext::new(server_tx)),
                    should_silently_fail: true,
                    ..Default::default()
                },
                None,
                None,
                InputMode::Normal,
                None,
                false,
                false,
            )
            .unwrap()
            .1
            .unwrap();
            result_tx.send(result).unwrap();
        });
        let ServerInstruction::ReconfigureAllClients {
            completion: Some(mut completion),
            ..
        } = server_rx.recv_timeout(Duration::from_secs(10)).unwrap().0
        else {
            panic!("expected a reconfiguration request");
        };
        // Hold the server's acknowledgement beyond the deadline that used to report success.
        assert!(matches!(
            result_rx.recv_timeout(ACTION_COMPLETION_TIMEOUT + Duration::from_millis(500)),
            Err(RecvTimeoutError::Timeout)
        ));
        if write_to_disk {
            completion.set_exit_status(1);
            completion.set_error_message("write failed".into());
        }
        drop(completion);
        let result = result_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(result.confirmed);
        assert_eq!(result.exit_status, write_to_disk.then_some(1));
        assert_eq!(
            result.error_message.as_deref(),
            write_to_disk.then_some("write failed")
        );
        route.join().unwrap();
    }
}

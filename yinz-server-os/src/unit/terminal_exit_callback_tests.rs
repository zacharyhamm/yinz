use super::*;

#[test]
fn terminal_exit_callback_preserves_id_status_and_command() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let callback = terminal_exit_callback(Box::new(move |id, status, command| {
        sender.send((id, status, command)).unwrap();
    }));
    let command = RunCommand {
        command: "/usr/bin/true".into(),
        args: vec!["argument".into()],
        cwd: Some("/tmp".into()),
        ..Default::default()
    };
    for status in [Some(17), None] {
        callback(TerminalId::new(42), status, command.clone());
        assert_eq!(
            receiver.recv().unwrap(),
            (
                PaneId::Terminal(TerminalId::new(42)),
                status,
                command.clone()
            )
        );
    }
}

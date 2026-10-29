use super::*;
use crate::route::NotificationEnd;

#[test]
fn paste_preprocesses_chunks_before_sending_one_write_with_its_completion() {
    let terminal = TerminalId::new(1);
    let pane = PaneId::Terminal(terminal);
    for (modes, expected) in [
        (b"".as_slice(), b"\r".as_slice()),
        (b"\x1b[20h".as_slice(), b"\r\n".as_slice()),
        (
            b"\x1b[?2004h".as_slice(),
            b"\x1b[200~\r\x1b[201~".as_slice(),
        ),
        (
            b"\x1b[?2004h\x1b[20h".as_slice(),
            b"\x1b[200~\r\n\x1b[201~".as_slice(),
        ),
    ] {
        let mut tab = create_new_tab(Size { cols: 80, rows: 20 }, true);
        tab.handle_pty_bytes(terminal, modes.to_vec()).unwrap();
        let writes = install_pty_writer_capture(&mut tab);
        let (tx, mut result) = tokio::sync::oneshot::channel();
        tab.paste_to_pane_id(b"\r".to_vec(), pane, None, Some(NotificationEnd::new(tx)))
            .unwrap();
        let mut instructions = drain_pty_writer(&writes);
        assert_eq!(instructions.len(), 1);
        let PtyWriteInstruction::Write(bytes, id, Some(mut completion)) =
            instructions.pop().unwrap()
        else {
            panic!("paste must own one write completion");
        };
        assert_eq!(id, terminal);
        assert_eq!(bytes, expected);
        assert!(matches!(
            result.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        completion.set_error_message("paste failed".into());
        drop(completion);
        assert_eq!(
            result.try_recv().unwrap().error_message.as_deref(),
            Some("paste failed")
        );
    }
}

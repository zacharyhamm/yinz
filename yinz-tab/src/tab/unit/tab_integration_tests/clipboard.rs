use super::*;
use yinz_bus::plugins::ClipboardHistoryEntry;
use yinz_types::data::CopyDestination;
use yinz_types::input::options::Clipboard;

fn history(receiver: &Receiver<(PluginInstruction, ErrorContext)>) -> Vec<ClipboardHistoryEntry> {
    receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::RecordClipboard(entry) => Some(entry),
            _ => None,
        })
        .collect()
}

#[test]
fn clipboard_history_records_explicit_copies_with_current_session_and_destination() {
    for (clipboard, destination) in [
        (Clipboard::System, CopyDestination::System),
        (Clipboard::Primary, CopyDestination::Primary),
    ] {
        let (mut tab, receiver) =
            create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
        tab.clipboard_provider = crate::tab::clipboard::ClipboardProvider::Osc52(clipboard);
        tab.senders.session.set("before".into());
        tab.copy_text_to_clipboard("héllo\0\n").unwrap();
        tab.senders.session.set("after".into());
        tab.copy_text_to_clipboard("héllo\0\n").unwrap();
        let records = history(&receiver);
        assert_eq!(records.len(), 2);
        for (entry, session) in records.iter().zip(["before", "after"]) {
            assert_eq!(entry.content, "héllo\0\n");
            assert_eq!(entry.destination, destination);
            assert_eq!(entry.yinz_session_name.as_str(), session);
            assert_eq!(entry.host_id, yinz_runtime::host_identity::host_id());
            assert!(entry.copied_at_ms > 0);
        }
    }
}

#[test]
fn clipboard_history_records_each_terminal_osc52_write_in_one_batch() {
    let (mut tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    tab.handle_pty_bytes(
        TerminalId::new(1),
        b"\x1b]52;c;YQ==\x07\x1b]52;c;YQ==\x07\x1b]52;c;\x07".to_vec(),
    )
    .unwrap();
    assert_eq!(
        history(&receiver)
            .iter()
            .map(|entry| entry.content.as_str())
            .collect::<Vec<_>>(),
        ["a", "a", ""]
    );
}

#[test]
fn clipboard_history_records_selection_copies() {
    let client = ClientId::new(1);
    let (mut tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    tab.handle_pty_bytes(TerminalId::new(1), b"selected".to_vec())
        .unwrap();
    let pane = tab.get_active_pane_or_floating_pane_mut(client).unwrap();
    pane.start_selection(&Position::new(0, 0), client);
    pane.end_selection(&Position::new(0, 8), client);
    tab.copy_selection(client).unwrap();
    let records = history(&receiver);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].content, "selected");
}

#[test]
fn clipboard_history_channel_failure_does_not_fail_the_copy() {
    let (tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    drop(receiver);
    tab.copy_text_to_clipboard("still copied").unwrap();
}

#[test]
fn clipboard_history_records_mouse_copy_on_select() {
    let client = ClientId::new(1);
    let (mut tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    tab.handle_pty_bytes(TerminalId::new(1), b"selected".to_vec())
        .unwrap();
    let pane = tab.get_active_pane(client).unwrap();
    let start = Position::new(pane.get_content_y() as i32, pane.get_content_x() as u16);
    let end = Position::new(
        pane.get_content_y() as i32,
        (pane.get_content_x() + 8) as u16,
    );
    for event in [
        yinz_types::mouse::MouseEvent::new_left_press_event(start),
        yinz_types::mouse::MouseEvent::new_left_motion_event(end),
        yinz_types::mouse::MouseEvent::new_left_release_event(end),
    ] {
        tab.handle_mouse_event(&event, client).unwrap();
    }
    let records = history(&receiver);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].content, "selected");
}

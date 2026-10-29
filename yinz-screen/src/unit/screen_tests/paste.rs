use super::*;
use crate::route::NotificationEnd;
use tokio::sync::oneshot;
use yinz_types::internal_ids::PluginId;
use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsLayout};

#[test]
fn client_paste_sends_one_preprocessed_write_with_completion() {
    let client = ClientId::new(1);
    let terminal = TerminalId::new(1);
    for mode in [InputMode::Normal, InputMode::Locked] {
        for bracketed in [false, true] {
            let (mut screen, capture) =
                create_new_screen_with_forward_capture(Size { cols: 80, rows: 20 });
            new_tab(&mut screen, 1, TabId::new(0));
            screen.change_mode(mode, None, client).unwrap();
            let tab = screen.get_active_tab_mut(client).unwrap();
            tab.handle_pty_bytes(terminal, b"line\r\n".repeat(50))
                .unwrap();
            tab.handle_pty_bytes(terminal, b"\x1b[20h".to_vec())
                .unwrap();
            if bracketed {
                tab.handle_pty_bytes(terminal, b"\x1b[?2004h".to_vec())
                    .unwrap();
            }
            tab.scroll_active_terminal_up(client);
            assert!(tab.active_pane_is_scrolled(client));
            screen.open_context_menu(
                yinz_types::data::ContextMenuContext {
                    pane_id: PaneId::Terminal(terminal),
                    click: (10, 5),
                    ..Default::default()
                },
                client,
            );
            assert!(screen.context_menu_is_open(client));
            capture.pty_writer_rx.try_iter().for_each(drop);
            let (tx, mut result) = oneshot::channel();
            screen
                .paste_from_client("\r".into(), client, Some(NotificationEnd::new(tx)))
                .unwrap();
            assert!(!screen.context_menu_is_open(client));
            assert!(!screen.active_pane_is_scrolled(client));
            let instructions: Vec<_> = capture.pty_writer_rx.try_iter().collect();
            assert_eq!(instructions.len(), 1);
            let (PtyWriteInstruction::Write(bytes, target, Some(mut completion)), _) =
                instructions.into_iter().next().unwrap()
            else {
                panic!("paste must own one write completion");
            };
            assert_eq!(target, terminal);
            let expected: &[u8] = if bracketed {
                b"\x1b[200~\r\n\x1b[201~"
            } else {
                b"\r\n"
            };
            assert_eq!(bytes, expected);
            assert!(matches!(
                result.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            completion.set_error_message("paste failed".into());
            drop(completion);
            assert_eq!(
                result.try_recv().unwrap().error_message.as_deref(),
                Some("paste failed")
            );
        }
    }
}

#[tokio::test]
async fn synchronized_client_paste_waits_for_each_target_and_skips_excluded_panes() {
    let client = ClientId::new(1);
    let (mut screen, capture) = create_new_screen_with_forward_capture(Size {
        cols: 120,
        rows: 24,
    });
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.get_active_tab_mut(client).unwrap();
    for id in [2, 3] {
        tab.new_pane(
            PaneId::Terminal(TerminalId::new(id)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(client),
            None,
        )
        .unwrap();
    }
    tab.get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(3)))
        .unwrap()
        .set_exclude_from_sync(true);
    tab.handle_pty_bytes(TerminalId::new(1), b"\x1b[?2004h\x1b[20h".to_vec())
        .unwrap();
    tab.toggle_sync_panes_is_active();
    capture.pty_writer_rx.try_iter().for_each(drop);
    let (tx, mut result) = oneshot::channel();
    screen
        .paste_from_client(
            "\r".into(),
            client,
            Some(NotificationEnd::new_client_input(tx)),
        )
        .unwrap();
    let mut writes: Vec<_> = capture
        .pty_writer_rx
        .try_iter()
        .map(|(instruction, _)| {
            let PtyWriteInstruction::Write(bytes, id, Some(completion)) = instruction else {
                panic!("each target must receive one write with completion");
            };
            (id, bytes, completion)
        })
        .collect();
    writes.sort_by_key(|(id, _, _)| *id);
    assert_eq!(writes.len(), 2);
    assert_eq!(
        (writes[0].0, writes[0].1.as_slice()),
        (TerminalId::new(1), b"\x1b[200~\r\n\x1b[201~".as_slice())
    );
    assert_eq!(
        (writes[1].0, writes[1].1.as_slice()),
        (TerminalId::new(2), b"\r".as_slice())
    );
    let (_, _, mut first) = writes.remove(0);
    first.set_error_message("first target failed".into());
    drop(first);
    assert!(matches!(
        result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    drop(writes);
    let result = tokio::time::timeout(yinz_test_support::default_timeout(), result)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.error_message.as_deref(), Some("first target failed"));
}

#[test]
fn client_paste_preserves_plugin_client_context_and_pane_tab_focus() {
    let client = ClientId::new(1);
    let plugin = PluginId::new(100);
    let pane = PaneId::Plugin(plugin);
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.get_active_tab_mut(client).unwrap();
    tab.new_pane(
        pane,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    let tabs = tab
        .mount_pane_tabs(
            pane,
            PaneTabsLayout {
                active: 0,
                tabs: vec![PaneTabDescriptor {
                    label: "Plugin".into(),
                    location: "yinz:strider".into(),
                    initial_cwd: None,
                    configuration: BTreeMap::new(),
                }],
            },
            0,
            client,
        )
        .unwrap();
    for bar_focused in [false, true] {
        screen
            .get_active_tab_mut(client)
            .unwrap()
            .focus_pane_tabs(tabs.id, bar_focused, client)
            .unwrap();
        capture.drain_plugin_updates();
        screen
            .paste_from_client("paste".into(), client, None)
            .unwrap();
        let pasted: Vec<_> = capture
            .drain_plugin_updates()
            .into_iter()
            .filter(|(_, _, event)| matches!(event, Event::PastedText(_)))
            .collect();
        let expected = if bar_focused {
            vec![]
        } else {
            vec![(
                Some(plugin),
                Some(client),
                Event::PastedText("paste".into()),
            )]
        };
        assert_eq!(pasted, expected);
        assert!(capture.drain_pty_writes().is_empty());
    }
}

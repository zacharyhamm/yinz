use super::*;

#[test]
fn kitty_query_replies_ok_when_capable_client_connected() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.update_kitty_graphics_support(1, true);
    assert_eq!(screen.kitty_host_capabilities.borrow().get(&1), Some(&true));
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
        vec![b"\x1b_Gi=31;OK\x1b\\".to_vec()]
    );
}

#[test]
fn kitty_query_replies_enotsupported_when_no_capable_client() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.update_kitty_graphics_support(1, false);
    assert_eq!(
        screen.kitty_host_capabilities.borrow().get(&1),
        Some(&false)
    );
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.drain_messages_to_pty();
    assert_eq!(replies.len(), 1);
    let reply = String::from_utf8(replies[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b_Gi=31;ENOTSUPPORTED"));
    assert!(reply.ends_with("\x1b\\"));
}

#[test]
fn kitty_query_is_ignored_when_the_protocol_is_disabled_in_the_config() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen_with_kitty_graphics(size, true, true, false);
    new_tab(&mut screen, 1, 0);
    screen.update_kitty_graphics_support(1, true);
    assert_eq!(
        screen.kitty_host_capabilities.borrow().get(&1),
        Some(&false),
        "a capable host must still be recorded as incapable when the protocol is disabled"
    );
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    assert!(
        active_pane.drain_messages_to_pty().is_empty(),
        "a disabled protocol must not reply to queries at all"
    );
}

#[test]
fn kitty_support_recomputed_on_client_detach() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");
    screen.update_kitty_graphics_support(1, false);
    screen.update_kitty_graphics_support(2, true);
    screen.remove_client(2).expect("TEST");
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.drain_messages_to_pty();
    assert_eq!(replies.len(), 1);
    let reply = String::from_utf8(replies[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b_Gi=31;ENOTSUPPORTED"));
    assert!(reply.ends_with("\x1b\\"));
}

#[test]
fn primary_da_advertises_sixel_when_capable_client_connected() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.update_sixel_support(1, true);
    assert_eq!(screen.sixel_host_capabilities.borrow().get(&1), Some(&true));
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
        vec![b"\x1b[?62;4;52c".to_vec()]
    );
}

#[test]
fn primary_da_omits_sixel_when_no_capable_client() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.update_sixel_support(1, false);
    assert_eq!(
        screen.sixel_host_capabilities.borrow().get(&1),
        Some(&false)
    );
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
        vec![b"\x1b[?62;52c".to_vec()]
    );
}

#[test]
fn sixel_support_recomputed_on_client_detach() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");
    screen.update_sixel_support(1, false);
    screen.update_sixel_support(2, true);
    screen.remove_client(2).expect("TEST");
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
        vec![b"\x1b[?62;52c".to_vec()]
    );
}

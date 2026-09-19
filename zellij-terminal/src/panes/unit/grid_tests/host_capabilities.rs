use super::*;

#[test]
fn primary_device_attributes_advertise_sixel_when_host_supports_it() {
    let mut grid = create_grid_with_content("");
    let mut vte_parser = vte::Parser::new();
    grid.update_sixel_host_support(true);
    vte_parser.advance(&mut grid, b"\x1b[c");
    assert_eq!(
        grid.pending_messages_to_pty,
        vec![b"\x1b[?62;4;52c".to_vec()]
    );
}

#[test]
fn primary_device_attributes_omit_sixel_when_host_does_not_support_it() {
    let mut grid = create_grid_with_content("");
    let mut vte_parser = vte::Parser::new();
    grid.update_sixel_host_support(false);
    vte_parser.advance(&mut grid, b"\x1b[c");
    assert_eq!(grid.pending_messages_to_pty, vec![b"\x1b[?62;52c".to_vec()]);
}

#[test]
fn xtsmgraphics_color_registers_report_failure_when_host_does_not_support_sixel() {
    let mut grid = create_grid_with_content("");
    let mut vte_parser = vte::Parser::new();
    grid.update_sixel_host_support(false);
    vte_parser.advance(&mut grid, b"\x1b[?1;1S");
    assert_eq!(grid.pending_messages_to_pty, vec![b"\x1b[?1;3;0S".to_vec()]);
}

#[test]
fn xtsmgraphics_geometry_reports_failure_when_host_does_not_support_sixel() {
    let mut grid = create_grid_with_content("");
    let mut vte_parser = vte::Parser::new();
    grid.update_sixel_host_support(false);
    vte_parser.advance(&mut grid, b"\x1b[?2;1S");
    assert_eq!(grid.pending_messages_to_pty, vec![b"\x1b[?2;3;0S".to_vec()]);
}

#[test]
fn osc_52_read_is_forwarded_to_the_host() {
    let mut grid = new_grid_for_forwarding_test();
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, b"\x1b]52;c;?\x07");
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "clipboard reads must never be answered locally"
    );
    assert_eq!(
        grid.pending_forwarded_queries,
        vec![crate::host_query::HostQuery::ClipboardContent {
            selection: 'c',
            terminator: crate::host_query::OscTerminator::Bel,
        }]
    );
}

#[test]
fn osc_52_read_keeps_the_selection_and_terminator_of_the_query() {
    let mut grid = new_grid_for_forwarding_test();
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, b"\x1b]52;p;?\x1b\\");
    assert_eq!(
        grid.pending_forwarded_queries,
        vec![crate::host_query::HostQuery::ClipboardContent {
            selection: 'p',
            terminator: crate::host_query::OscTerminator::St,
        }]
    );
}

#[test]
fn osc_52_write_is_not_forwarded() {
    let mut grid = new_grid_for_forwarding_test();
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, b"\x1b]52;c;aGVsbG8=\x07");
    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "copying to the clipboard must not go through the forward path"
    );
    assert_eq!(grid.pending_clipboard_update.as_deref(), Some("hello"));
}

#[test]
fn xtgettcap_answers_the_ms_capability() {
    let mut grid = create_grid_with_content("");
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, b"\x1bP+q4d73\x1b\\");
    assert_eq!(
        grid.pending_messages_to_pty,
        vec![b"\x1bP1+r4D73=1B5D35323B25703125733B257032257307\x1b\\".to_vec()]
    );
}

#[test]
fn xtgettcap_rejects_unknown_capabilities() {
    let mut grid = create_grid_with_content("");
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, b"\x1bP+q62656c\x1b\\");
    assert_eq!(
        grid.pending_messages_to_pty,
        vec![b"\x1bP0+r\x1b\\".to_vec()]
    );
}

#[test]
fn xtgettcap_answers_each_requested_capability() {
    let mut grid = create_grid_with_content("");
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, b"\x1bP+q62656c;4d73\x1b\\");
    assert_eq!(grid.pending_messages_to_pty.len(), 2);
    assert_eq!(grid.pending_messages_to_pty[0], b"\x1bP0+r\x1b\\".to_vec());
    assert!(String::from_utf8_lossy(&grid.pending_messages_to_pty[1]).starts_with("\x1bP1+r4D73="));
}

#[test]
fn xtgettcap_rejects_malformed_hex() {
    let mut grid = create_grid_with_content("");
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, b"\x1bP+q4d7\x1b\\");
    assert_eq!(
        grid.pending_messages_to_pty,
        vec![b"\x1bP0+r\x1b\\".to_vec()]
    );
}

#[test]
fn sixel_dcs_is_not_confused_with_xtgettcap() {
    let mut grid = new_grid_for_forwarding_test();
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, b"\x1bPq#0;2;0;0;0\x1b\\");
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "a sixel image must not produce an XTGETTCAP reply"
    );
}

#[test]
fn disabling_the_kitty_keyboard_protocol_forgets_a_negotiation_and_re_enabling_waits_for_a_new_one()
{
    let mut grid = create_grid_with_content("");
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, b"\x1b[>1u");
    assert!(grid.supports_kitty_keyboard_protocol);

    grid.set_kitty_keyboard_protocol_disabled(true);
    assert!(!grid.supports_kitty_keyboard_protocol);
    vte_parser.advance(&mut grid, b"\x1b[>1u");
    assert!(
        !grid.supports_kitty_keyboard_protocol,
        "an app cannot negotiate the protocol back while it is off"
    );

    grid.set_kitty_keyboard_protocol_disabled(false);
    assert!(
        !grid.supports_kitty_keyboard_protocol,
        "turning it back on waits for the app to ask again"
    );
    vte_parser.advance(&mut grid, b"\x1b[>1u");
    assert!(grid.supports_kitty_keyboard_protocol);
}

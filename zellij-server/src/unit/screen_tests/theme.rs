use super::*;

#[test]
fn reconfigure_sends_a_plugin_pane_the_updated_client_mode_info_once() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, 0);
    screen.mode_info.insert(1, ModeInfo::default());
    add_pane_to_tab(&mut screen, 0, PaneId::Plugin(77), false);
    capture.drain_plugin_updates();

    screen
        .reconfigure(
            Keybinds::default(),
            InputMode::Locked,
            Styling::default(),
            true,
            None,
            PaneFrameStyle::None,
            None,
            None,
            false,
            true,
            false,
            true,
            true,
            false,
            None,
            true,
            true,
            true,
            true,
            false,
            false,
            true,
            DEFAULT_WORD_SEPARATORS.to_owned(),
            NestedSessionHandling::default(),
            false,
            1,
        )
        .unwrap();

    let mode_infos: Vec<_> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(
            |(plugin_id, client_id, event)| match (plugin_id, client_id, event) {
                (Some(77), Some(1), Event::ModeUpdate(mode_info)) => Some(mode_info),
                _ => None,
            },
        )
        .collect();
    assert_eq!(mode_infos.len(), 1);
    assert_eq!(mode_infos[0].base_mode, Some(InputMode::Locked));
    assert!(mode_infos[0].capabilities.arrow_fonts);
    assert!(mode_infos[0].style.hide_session_name);
}

#[test]
fn group_marking_updates_a_plugin_pane_s_mode_info_once() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, 0);
    add_pane_to_tab(&mut screen, 0, PaneId::Plugin(77), false);
    capture.drain_plugin_updates();

    screen.toggle_group_marking(1).unwrap();

    let mode_infos: Vec<_> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(
            |(plugin_id, client_id, event)| match (plugin_id, client_id, event) {
                (Some(77), Some(1), Event::ModeUpdate(mode_info)) => Some(mode_info),
                _ => None,
            },
        )
        .collect();
    assert_eq!(mode_infos.len(), 1);
    assert_eq!(mode_infos[0].currently_marking_pane_group, Some(true));
}

#[test]
fn host_theme_first_update_emits_plugin_event() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("update ok");

    let events = capture.drain_plugin_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::HostTerminalThemeChanged(zellij_utils::data::HostTerminalThemeMode::Dark)
        )),
        "Event::HostTerminalThemeChanged(Dark) must be fanned out, got: {:?}",
        events
    );
    assert_eq!(
        screen.host_terminal_theme_mode,
        Some(zellij_utils::data::HostTerminalThemeMode::Dark),
        "stored mode must be updated"
    );
}

#[test]
fn host_theme_dedupes_duplicate_mode() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Light)
        .expect("first ok");
    let _ = capture.drain_plugin_events();

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Light)
        .expect("second ok");

    let events = capture.drain_plugin_events();
    assert!(
        events.is_empty(),
        "duplicate mode must not re-emit any plugin events, got: {:?}",
        events
    );
}

#[test]
fn host_theme_emits_again_on_mode_flip() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("first ok");
    let _ = capture.drain_plugin_events();

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Light)
        .expect("flip ok");

    let events = capture.drain_plugin_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::HostTerminalThemeChanged(zellij_utils::data::HostTerminalThemeMode::Light)
        )),
        "mode flip must re-emit the plugin event, got: {:?}",
        events
    );
}

#[test]
fn color_palette_mode_query_short_circuits_to_dark_reply() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(zellij_utils::data::HostTerminalThemeMode::Dark);

    let token = screen.forward_host_query(PaneId::Terminal(13), HostQuery::ColorPaletteMode);

    assert_eq!(
        token, 0,
        "ColorPaletteMode must return the sentinel token; no real forward was queued"
    );
    assert!(
        capture.drain_forward_queries().is_empty(),
        "must NOT forward to host — Zellij answers from cache"
    );
    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "exactly one pty reply expected");
    assert_eq!(writes[0], (b"\x1b[?997;1n".to_vec(), 13));
    assert!(
        screen.forward_in_flight_token.is_none(),
        "slot must remain free; short-circuit does not occupy the queue"
    );
}

#[test]
fn color_palette_mode_query_short_circuits_to_light_reply() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(zellij_utils::data::HostTerminalThemeMode::Light);

    let _ = screen.forward_host_query(PaneId::Terminal(4), HostQuery::ColorPaletteMode);

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (b"\x1b[?997;2n".to_vec(), 4));
}

#[test]
fn color_palette_mode_query_falls_back_to_own_dark_theme_when_host_mode_unknown() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    assert!(
        screen.host_terminal_theme_mode.is_none(),
        "precondition: no host mode learned yet"
    );
    screen.style.colors.text_unselected.background =
        zellij_utils::data::PaletteColor::Rgb((0, 0, 0));

    let token = screen.forward_host_query(PaneId::Terminal(1), HostQuery::ColorPaletteMode);

    assert_eq!(
        token, 0,
        "ColorPaletteMode must return the sentinel token; no real forward was queued"
    );
    assert!(
        capture.drain_forward_queries().is_empty(),
        "must NOT forward to host — Zellij answers from its own effective theme"
    );
    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "exactly one pty reply expected");
    assert_eq!(writes[0], (b"\x1b[?997;1n".to_vec(), 1));
}

#[test]
fn color_palette_mode_query_falls_back_to_own_light_theme_when_host_mode_unknown() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    assert!(
        screen.host_terminal_theme_mode.is_none(),
        "precondition: no host mode learned yet"
    );
    screen.style.colors.text_unselected.background =
        zellij_utils::data::PaletteColor::Rgb((255, 255, 255));

    let _ = screen.forward_host_query(PaneId::Terminal(7), HostQuery::ColorPaletteMode);

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (b"\x1b[?997;2n".to_vec(), 7));
}

#[test]
fn color_palette_mode_query_prefers_known_host_mode_over_own_theme() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(zellij_utils::data::HostTerminalThemeMode::Light);
    screen.style.colors.text_unselected.background =
        zellij_utils::data::PaletteColor::Rgb((0, 0, 0));

    let _ = screen.forward_host_query(PaneId::Terminal(9), HostQuery::ColorPaletteMode);

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0],
        (b"\x1b[?997;2n".to_vec(), 9),
        "the host's announced mode must win over the local theme fallback"
    );
}

#[test]
fn color_palette_mode_query_skips_plugin_panes() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(zellij_utils::data::HostTerminalThemeMode::Dark);

    let _ = screen.forward_host_query(PaneId::Plugin(99), HostQuery::ColorPaletteMode);

    assert!(
        capture.drain_pty_writes().is_empty(),
        "plugin panes have no VT pty — they get Event::HostTerminalThemeChanged instead"
    );
}

#[test]
fn host_theme_no_pty_writes_when_no_panes_subscribed() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert!(
        writes.is_empty(),
        "no panes exist (or none subscribed via CSI ?2031h), so no DSR forward is queued, got: {:?}",
        writes
    );
}

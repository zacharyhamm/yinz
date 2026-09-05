use super::*;

#[test]
fn reconfigure_sends_a_plugin_pane_the_updated_client_mode_info_once() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, 0);
    screen
        .mode_info
        .insert(crate::ClientId(1), ModeInfo::default());
    add_pane_to_tab(&mut screen, 0, PaneId::Plugin(crate::PluginId(77)), false);
    capture.drain_plugin_updates();

    screen
        .reconfigure(
            Keybinds::default(),
            InputMode::Locked,
            Styling::default(),
            true,
            None,
            None, // default_cwd
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
            false, // context_menu_on_right_click
            ContextMenu::default(),
            true,
            DEFAULT_WORD_SEPARATORS.to_owned(),
            NestedSessionHandling::default(),
            false,
            HostNotificationProtocol::default(),
            Size { rows: 0, cols: 0 },
            crate::ClientId(1),
        )
        .unwrap();

    let mode_infos: Vec<_> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(
            |(plugin_id, client_id, event)| match (plugin_id, client_id, event) {
                (
                    Some(zellij_utils::internal_ids::PluginId(77)),
                    Some(ClientId(1)),
                    Event::ModeUpdate(mode_info),
                ) => Some(mode_info),
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
    add_pane_to_tab(&mut screen, 0, PaneId::Plugin(crate::PluginId(77)), false);
    capture.drain_plugin_updates();

    screen.toggle_group_marking(crate::ClientId(1)).unwrap();

    let mode_infos: Vec<_> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(
            |(plugin_id, client_id, event)| match (plugin_id, client_id, event) {
                (
                    Some(zellij_utils::internal_ids::PluginId(77)),
                    Some(ClientId(1)),
                    Event::ModeUpdate(mode_info),
                ) => Some(mode_info),
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

    let token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId(13)),
        HostQuery::ColorPaletteMode,
    );

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
    assert_eq!(writes[0], (b"\x1b[?997;1n".to_vec(), TerminalId(13)));
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

    let _ = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId(4)),
        HostQuery::ColorPaletteMode,
    );

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (b"\x1b[?997;2n".to_vec(), TerminalId(4)));
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

    let token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId(1)),
        HostQuery::ColorPaletteMode,
    );

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
    assert_eq!(writes[0], (b"\x1b[?997;1n".to_vec(), TerminalId(1)));
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

    let _ = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId(7)),
        HostQuery::ColorPaletteMode,
    );

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (b"\x1b[?997;2n".to_vec(), TerminalId(7)));
}

#[test]
fn color_palette_mode_query_prefers_known_host_mode_over_own_theme() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(zellij_utils::data::HostTerminalThemeMode::Light);
    screen.style.colors.text_unselected.background =
        zellij_utils::data::PaletteColor::Rgb((0, 0, 0));

    let _ = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId(9)),
        HostQuery::ColorPaletteMode,
    );

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0],
        (b"\x1b[?997;2n".to_vec(), TerminalId(9)),
        "the host's announced mode must win over the local theme fallback"
    );
}

#[test]
fn color_palette_mode_query_skips_plugin_panes() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(zellij_utils::data::HostTerminalThemeMode::Dark);

    let _ = screen.forward_host_query(
        PaneId::Plugin(crate::PluginId(99)),
        HostQuery::ColorPaletteMode,
    );

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

fn styling_with_background(color: (u8, u8, u8)) -> zellij_utils::data::Styling {
    let mut styling = zellij_utils::data::Styling::default();
    styling.text_unselected.background = zellij_utils::data::PaletteColor::Rgb(color);
    styling
}

const TEST_DARK_BG: (u8, u8, u8) = (17, 17, 17);
const TEST_LIGHT_BG: (u8, u8, u8) = (238, 238, 238);

fn create_new_screen_with_dark_and_light_themes(size: Size) -> (Screen, ThemeCapture) {
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    screen.host_theme_dark_styling = Some(styling_with_background(TEST_DARK_BG));
    screen.host_theme_light_styling = Some(styling_with_background(TEST_LIGHT_BG));
    (screen, capture)
}

#[test]
fn explicit_theme_hue_resolves_the_session_appearance() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_dark_and_light_themes(size);

    screen
        .apply_configured_explicit_theme_hue(Some(zellij_utils::data::ThemeHue::Light))
        .expect("explicit hue applied");

    assert_eq!(
        screen.host_terminal_theme_mode,
        Some(zellij_utils::data::HostTerminalThemeMode::Light),
        "an explicit hue must become the session's effective mode"
    );
    assert_eq!(
        screen.style.colors.text_unselected.background,
        zellij_utils::data::PaletteColor::Rgb(TEST_LIGHT_BG),
        "Screen's own style must track the swap so later panes inherit it"
    );
    let events = capture.drain_plugin_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::HostTerminalThemeChanged(zellij_utils::data::HostTerminalThemeMode::Light)
        )),
        "plugins must learn the resolved mode, got: {:?}",
        events
    );
}

#[test]
fn dark_and_light_themes_without_an_explicit_hue_default_to_dark() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_dark_and_light_themes(size);

    screen
        .resolve_default_theme_mode()
        .expect("default resolved");

    assert_eq!(
        screen.host_terminal_theme_mode,
        Some(zellij_utils::data::HostTerminalThemeMode::Dark),
    );
    assert_eq!(
        screen.style.colors.text_unselected.background,
        zellij_utils::data::PaletteColor::Rgb(TEST_DARK_BG),
        "a configured dark/light pair must resolve to the dark theme rather than \
         to the unrelated static theme"
    );
    assert!(
        capture.drain_plugin_events().iter().any(|e| matches!(
            e,
            Event::HostTerminalThemeChanged(zellij_utils::data::HostTerminalThemeMode::Dark)
        )),
        "the resolved mode is real state and must reach plugins"
    );
}

#[test]
fn the_default_dark_mode_still_yields_to_the_host_terminal() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .resolve_default_theme_mode()
        .expect("default resolved");

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Light)
        .expect("host report applied");

    assert_eq!(
        screen.host_terminal_theme_mode,
        Some(zellij_utils::data::HostTerminalThemeMode::Light),
        "the default is not a pin - the host terminal remains authoritative"
    );
    assert_eq!(
        screen.style.colors.text_unselected.background,
        zellij_utils::data::PaletteColor::Rgb(TEST_LIGHT_BG),
    );
}

#[test]
fn no_default_mode_without_both_themes_configured() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    screen.host_theme_dark_styling = Some(styling_with_background(TEST_DARK_BG));

    screen
        .resolve_default_theme_mode()
        .expect("nothing to resolve");

    assert_eq!(
        screen.host_terminal_theme_mode, None,
        "a lone theme_dark leaves the static theme authoritative, so the mode \
         stays genuinely unknown"
    );
    assert!(
        capture.drain_plugin_events().is_empty(),
        "no synthetic event may be emitted while the mode is unknown"
    );
}

#[test]
fn the_default_does_not_override_an_explicit_hue() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .apply_configured_explicit_theme_hue(Some(zellij_utils::data::ThemeHue::Light))
        .expect("explicit hue applied");

    screen
        .resolve_default_theme_mode()
        .expect("default is a no-op here");

    assert_eq!(
        screen.host_terminal_theme_mode,
        Some(zellij_utils::data::HostTerminalThemeMode::Light),
    );
}

#[test]
fn explicit_theme_hue_outranks_ambient_host_reports() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .apply_configured_explicit_theme_hue(Some(zellij_utils::data::ThemeHue::Light))
        .expect("explicit hue applied");
    let _ = capture.drain_plugin_events();

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("ambient report absorbed");

    assert_eq!(
        screen.host_terminal_theme_mode,
        Some(zellij_utils::data::HostTerminalThemeMode::Light),
        "the pinned mode must survive an ambient report to the contrary"
    );
    assert_eq!(
        screen.style.colors.text_unselected.background,
        zellij_utils::data::PaletteColor::Rgb(TEST_LIGHT_BG),
        "the palette must not be swapped while pinned"
    );
    assert!(
        capture.drain_plugin_events().is_empty(),
        "a suppressed report is not a mode change and must not reach plugins"
    );
}

#[test]
fn manual_theme_action_pins_the_session() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_dark_and_light_themes(size);
    let mut completion_tx = None;

    screen
        .apply_manual_host_terminal_theme_mode(
            zellij_utils::data::HostTerminalThemeMode::Dark,
            &mut completion_tx,
        )
        .expect("manual switch ok");
    let _ = capture.drain_plugin_events();

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Light)
        .expect("ambient report absorbed");

    assert_eq!(
        screen.host_terminal_theme_mode,
        Some(zellij_utils::data::HostTerminalThemeMode::Dark),
        "a deliberate choice must not be reverted by the host terminal"
    );
    assert!(
        capture.drain_plugin_events().is_empty(),
        "no mode change occurred, so no plugin event may be emitted"
    );
}

#[test]
fn removing_explicit_theme_hue_hands_authority_back_to_the_host() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .apply_configured_explicit_theme_hue(Some(zellij_utils::data::ThemeHue::Light))
        .expect("explicit hue applied");
    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("ambient report recorded while pinned");
    let _ = capture.drain_plugin_events();

    screen
        .apply_configured_explicit_theme_hue(None)
        .expect("unpinned");

    assert_eq!(
        screen.host_terminal_theme_mode,
        Some(zellij_utils::data::HostTerminalThemeMode::Dark),
        "unpinning must restore the ambient report that was suppressed"
    );
    assert_eq!(
        screen.style.colors.text_unselected.background,
        zellij_utils::data::PaletteColor::Rgb(TEST_DARK_BG),
        "the palette must follow the restored mode"
    );
}

#[test]
fn pinning_the_current_hue_repaints_the_resolved_palette() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("host report applied");
    screen.style.colors = styling_with_background((1, 2, 3));

    screen
        .apply_configured_explicit_theme_hue(Some(zellij_utils::data::ThemeHue::Dark))
        .expect("explicit hue applied");

    assert_eq!(
        screen.style.colors.text_unselected.background,
        zellij_utils::data::PaletteColor::Rgb(TEST_DARK_BG),
        "pinning the hue the session is already in must still resolve the palette, \
         otherwise a reconfigure leaves the session painted with the static theme"
    );
}

#[test]
fn unpinning_back_to_the_current_hue_repaints_the_resolved_palette() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("host report applied");
    screen
        .apply_configured_explicit_theme_hue(Some(zellij_utils::data::ThemeHue::Dark))
        .expect("explicit hue applied");
    screen.style.colors = styling_with_background((1, 2, 3));

    screen
        .apply_configured_explicit_theme_hue(None)
        .expect("unpinned");

    assert_eq!(
        screen.style.colors.text_unselected.background,
        zellij_utils::data::PaletteColor::Rgb(TEST_DARK_BG),
        "unpinning to the mode the session is already in must still resolve the palette"
    );
}

#[test]
fn effective_theme_mode_is_reasserted_after_a_theme_definition_change() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Light)
        .expect("host report applied");

    let new_light = styling_with_background((250, 250, 250));
    screen.host_theme_light_styling = Some(new_light);
    screen.style.colors = styling_with_background((1, 2, 3));

    screen
        .reapply_effective_theme_mode()
        .expect("mode reasserted");

    assert_eq!(
        screen.style.colors.text_unselected.background,
        zellij_utils::data::PaletteColor::Rgb((250, 250, 250)),
        "the session's mode must be re-resolved against the new definitions \
         instead of falling back to the static theme"
    );
}

#[test]
fn color_palette_mode_query_answers_with_the_pinned_hue() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen
        .apply_configured_explicit_theme_hue(Some(zellij_utils::data::ThemeHue::Light))
        .expect("explicit hue applied");
    // A contrary host report is recorded but must not leak into the answer,
    // and neither may a guess from the palette's (dark) background.
    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("ambient report absorbed");
    screen.style.colors.text_unselected.background =
        zellij_utils::data::PaletteColor::Rgb((0, 0, 0));
    let _ = capture.drain_pty_writes();

    let _ = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId(3)),
        HostQuery::ColorPaletteMode,
    );

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "exactly one pty reply expected");
    assert_eq!(
        writes[0],
        (b"\x1b[?997;2n".to_vec(), TerminalId(3)),
        "a pinned session tells apps the pinned hue, not the host's or a guess"
    );
}

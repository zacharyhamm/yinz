#[cfg(test)]
const CLIENT_1: yinz_types::internal_ids::ClientId = yinz_types::internal_ids::ClientId::new(1);
#[cfg(test)]
const PLUGIN_77: yinz_types::internal_ids::PluginId = yinz_types::internal_ids::PluginId::new(77);

use super::*;
use yinz_types::internal_ids::TabId;

#[test]
fn reconfigure_sends_a_plugin_pane_the_updated_client_mode_info_once() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .core
        .mode_info
        .insert(crate::ClientId::new(1), ModeInfo::default());
    add_pane_to_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Plugin(crate::PluginId::new(77)),
        false,
    );
    capture.drain_plugin_updates();

    let mut config = test_screen_config();
    config.theme = Styling::default();
    config.simplified_ui = true;
    config.default_mode = Some(InputMode::Locked);
    config.copy_on_select = false;
    config.hide_session_name = true;
    screen
        .reconfigure(config, Keybinds::default(), crate::ClientId::new(1))
        .unwrap();

    let mode_infos: Vec<_> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(
            |(plugin_id, client_id, event)| match (plugin_id, client_id, event) {
                (Some(PLUGIN_77), Some(CLIENT_1), Event::ModeUpdate(mode_info)) => Some(mode_info),
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
    new_tab(&mut screen, 1, TabId::new(0));
    add_pane_to_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Plugin(crate::PluginId::new(77)),
        false,
    );
    capture.drain_plugin_updates();

    screen
        .toggle_group_marking(crate::ClientId::new(1))
        .unwrap();

    let mode_infos: Vec<_> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(
            |(plugin_id, client_id, event)| match (plugin_id, client_id, event) {
                (Some(PLUGIN_77), Some(CLIENT_1), Event::ModeUpdate(mode_info)) => Some(mode_info),
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
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Dark)
        .expect("update ok");

    let events = capture.drain_plugin_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::HostTerminalThemeChanged(yinz_types::style::HostTerminalThemeMode::Dark)
        )),
        "Event::HostTerminalThemeChanged(Dark) must be fanned out, got: {:?}",
        events
    );
    assert_eq!(
        screen.core.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Dark),
        "stored mode must be updated"
    );
}

#[test]
fn host_theme_dedupes_duplicate_mode() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);

    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Light)
        .expect("first ok");
    let _ = capture.drain_plugin_events();

    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Light)
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
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Dark)
        .expect("first ok");
    let _ = capture.drain_plugin_events();

    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Light)
        .expect("flip ok");

    let events = capture.drain_plugin_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::HostTerminalThemeChanged(yinz_types::style::HostTerminalThemeMode::Light)
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
    screen.core.host_terminal_theme_mode = Some(yinz_types::style::HostTerminalThemeMode::Dark);

    let token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(13)),
        HostQuery::ColorPaletteMode,
    );

    assert_eq!(
        token, 0,
        "ColorPaletteMode must return the sentinel token; no real forward was queued"
    );
    assert!(
        capture.drain_forward_queries().is_empty(),
        "must NOT forward to host — Yinz answers from cache"
    );
    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "exactly one pty reply expected");
    assert_eq!(writes[0], (b"\x1b[?997;1n".to_vec(), TerminalId::new(13)));
    assert!(
        screen.core.forward_in_flight_token.is_none(),
        "slot must remain free; short-circuit does not occupy the queue"
    );
}

#[test]
fn color_palette_mode_query_short_circuits_to_the_baseline_light_reply() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let mut light = terminal_block(0);
    light.background = (250, 250, 250);
    screen
        .set_application_theme_config(static_application_theme(Some(light)))
        .unwrap();
    // A viewer's report does not change the answer.
    screen.core.host_terminal_theme_mode = Some(yinz_types::style::HostTerminalThemeMode::Dark);

    let _ = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(4)),
        HostQuery::ColorPaletteMode,
    );

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (b"\x1b[?997;2n".to_vec(), TerminalId::new(4)));
}

#[test]
fn color_palette_mode_query_falls_back_to_own_dark_theme_when_host_mode_unknown() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    assert!(
        screen.core.host_terminal_theme_mode.is_none(),
        "precondition: no host mode learned yet"
    );
    screen.core.style.colors.text_unselected.background =
        yinz_types::colors::PaletteColor::Rgb((0, 0, 0));

    let token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(1)),
        HostQuery::ColorPaletteMode,
    );

    assert_eq!(
        token, 0,
        "ColorPaletteMode must return the sentinel token; no real forward was queued"
    );
    assert!(
        capture.drain_forward_queries().is_empty(),
        "must NOT forward to host — Yinz answers from its own effective theme"
    );
    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "exactly one pty reply expected");
    assert_eq!(writes[0], (b"\x1b[?997;1n".to_vec(), TerminalId::new(1)));
}

#[test]
fn color_palette_mode_query_follows_the_session_static_theme_hue_not_the_chrome() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    assert!(
        screen.core.host_terminal_theme_mode.is_none(),
        "precondition: no host mode learned yet"
    );
    // A viewer's dark chrome is not the session theme.
    screen.core.style.colors.text_unselected.background =
        yinz_types::colors::PaletteColor::Rgb((0, 0, 0));
    screen
        .set_application_theme_config(yinz_config::screen_config::ApplicationThemeConfig {
            static_background: yinz_types::colors::PaletteColor::Rgb((255, 255, 255)),
            ..static_application_theme(None)
        })
        .unwrap();

    let _ = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(7)),
        HostQuery::ColorPaletteMode,
    );

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (b"\x1b[?997;2n".to_vec(), TerminalId::new(7)));
}

#[test]
fn color_palette_mode_query_ignores_a_viewers_reported_host_mode() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.core.host_terminal_theme_mode = Some(yinz_types::style::HostTerminalThemeMode::Light);
    screen.core.style.colors.text_unselected.background =
        yinz_types::colors::PaletteColor::Rgb((0, 0, 0));

    let _ = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(9)),
        HostQuery::ColorPaletteMode,
    );

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0],
        (b"\x1b[?997;1n".to_vec(), TerminalId::new(9)),
        "applications are told the session baseline, not the last viewer's mode"
    );
}

#[test]
fn color_palette_mode_query_skips_plugin_panes() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.core.host_terminal_theme_mode = Some(yinz_types::style::HostTerminalThemeMode::Dark);

    let _ = screen.forward_host_query(
        PaneId::Plugin(crate::PluginId::new(99)),
        HostQuery::ColorPaletteMode,
    );

    assert!(
        capture.drain_pty_writes().is_empty(),
        "plugin panes have no VT pty — they get Event::HostTerminalThemeChanged instead"
    );
}

#[test]
fn host_theme_reports_write_no_dsr_into_opted_in_panes() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    let pane = screen
        .core
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .get_pane_with_id_mut(pane_id)
        .unwrap();
    pane.terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b[?2031h".to_vec());
    let _ = capture.drain_pty_writes();

    for mode in [
        yinz_types::style::HostTerminalThemeMode::Light,
        yinz_types::style::HostTerminalThemeMode::Dark,
    ] {
        screen.update_host_terminal_theme_mode(mode).expect("ok");
    }

    let writes = capture.drain_pty_writes();
    assert!(
        !writes
            .iter()
            .any(|(bytes, _)| bytes.starts_with(b"\x1b[?997;")),
        "a viewer's appearance report must not reach the shared PTY, got: {writes:?}"
    );
    assert!(capture
        .drain_plugin_events()
        .iter()
        .any(|e| matches!(e, Event::HostTerminalThemeChanged(_))));
}

fn styling_with_background(color: (u8, u8, u8)) -> yinz_types::style::Styling {
    let mut styling = yinz_types::style::Styling::default();
    styling.text_unselected.background = yinz_types::colors::PaletteColor::Rgb(color);
    styling
}

const TEST_DARK_BG: (u8, u8, u8) = (17, 17, 17);
const TEST_LIGHT_BG: (u8, u8, u8) = (238, 238, 238);

fn create_new_screen_with_dark_and_light_themes(size: Size) -> (Screen, ThemeCapture) {
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    screen.core.host_theme_dark_styling = Some(styling_with_background(TEST_DARK_BG));
    screen.core.host_theme_light_styling = Some(styling_with_background(TEST_LIGHT_BG));
    // The session theme defines the same pair, the gate for manual pins.
    screen
        .set_application_theme_config(paired_application_theme(None, None))
        .unwrap();
    (screen, capture)
}

#[test]
fn explicit_theme_hue_resolves_the_session_appearance() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_dark_and_light_themes(size);

    screen
        .apply_configured_explicit_theme_hue(Some(yinz_types::style::ThemeHue::Light))
        .expect("explicit hue applied");

    assert_eq!(
        screen.core.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Light),
        "an explicit hue must become the session's effective mode"
    );
    assert_eq!(
        screen.core.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb(TEST_LIGHT_BG),
        "Screen's own style must track the swap so later panes inherit it"
    );
    let events = capture.drain_plugin_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::HostTerminalThemeChanged(yinz_types::style::HostTerminalThemeMode::Light)
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
        screen.core.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Dark),
    );
    assert_eq!(
        screen.core.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb(TEST_DARK_BG),
        "a configured dark/light pair must resolve to the dark theme rather than \
         to the unrelated static theme"
    );
    assert!(
        capture.drain_plugin_events().iter().any(|e| matches!(
            e,
            Event::HostTerminalThemeChanged(yinz_types::style::HostTerminalThemeMode::Dark)
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
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Light)
        .expect("host report applied");

    assert_eq!(
        screen.core.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Light),
        "the default is not a pin - the host terminal remains authoritative"
    );
    assert_eq!(
        screen.core.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb(TEST_LIGHT_BG),
    );
}

#[test]
fn no_default_mode_without_both_themes_configured() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    screen.core.host_theme_dark_styling = Some(styling_with_background(TEST_DARK_BG));

    screen
        .resolve_default_theme_mode()
        .expect("nothing to resolve");

    assert_eq!(
        screen.core.host_terminal_theme_mode, None,
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
        .apply_configured_explicit_theme_hue(Some(yinz_types::style::ThemeHue::Light))
        .expect("explicit hue applied");

    screen
        .resolve_default_theme_mode()
        .expect("default is a no-op here");

    assert_eq!(
        screen.core.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Light),
    );
}

#[test]
fn explicit_theme_hue_outranks_ambient_host_reports() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .apply_configured_explicit_theme_hue(Some(yinz_types::style::ThemeHue::Light))
        .expect("explicit hue applied");
    let _ = capture.drain_plugin_events();

    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Dark)
        .expect("ambient report absorbed");

    assert_eq!(
        screen.core.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Light),
        "the pinned mode must survive an ambient report to the contrary"
    );
    assert_eq!(
        screen.core.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb(TEST_LIGHT_BG),
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
            yinz_types::style::HostTerminalThemeMode::Dark,
            &mut completion_tx,
        )
        .expect("manual switch ok");
    let _ = capture.drain_plugin_events();

    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Light)
        .expect("ambient report absorbed");

    assert_eq!(
        screen.core.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Dark),
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
        .apply_configured_explicit_theme_hue(Some(yinz_types::style::ThemeHue::Light))
        .expect("explicit hue applied");
    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Dark)
        .expect("ambient report recorded while pinned");
    let _ = capture.drain_plugin_events();

    screen
        .apply_configured_explicit_theme_hue(None)
        .expect("unpinned");

    assert_eq!(
        screen.core.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Dark),
        "unpinning must restore the ambient report that was suppressed"
    );
    assert_eq!(
        screen.core.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb(TEST_DARK_BG),
        "the palette must follow the restored mode"
    );
}

#[test]
fn pinning_the_current_hue_repaints_the_resolved_palette() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Dark)
        .expect("host report applied");
    screen.core.style.colors = styling_with_background((1, 2, 3));

    screen
        .apply_configured_explicit_theme_hue(Some(yinz_types::style::ThemeHue::Dark))
        .expect("explicit hue applied");

    assert_eq!(
        screen.core.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb(TEST_DARK_BG),
        "pinning the hue the session is already in must still resolve the palette, \
         otherwise a reconfigure leaves the session painted with the static theme"
    );
}

#[test]
fn unpinning_back_to_the_current_hue_repaints_the_resolved_palette() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Dark)
        .expect("host report applied");
    screen
        .apply_configured_explicit_theme_hue(Some(yinz_types::style::ThemeHue::Dark))
        .expect("explicit hue applied");
    screen.core.style.colors = styling_with_background((1, 2, 3));

    screen
        .apply_configured_explicit_theme_hue(None)
        .expect("unpinned");

    assert_eq!(
        screen.core.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb(TEST_DARK_BG),
        "unpinning to the mode the session is already in must still resolve the palette"
    );
}

#[test]
fn effective_theme_mode_is_reasserted_after_a_theme_definition_change() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_dark_and_light_themes(size);
    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Light)
        .expect("host report applied");

    let new_light = styling_with_background((250, 250, 250));
    screen.core.host_theme_light_styling = Some(new_light);
    screen.core.style.colors = styling_with_background((1, 2, 3));

    screen
        .reapply_effective_theme_mode()
        .expect("mode reasserted");

    assert_eq!(
        screen.core.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb((250, 250, 250)),
        "the session's mode must be re-resolved against the new definitions \
         instead of falling back to the static theme"
    );
}

#[test]
fn color_palette_mode_query_answers_with_the_pinned_hue() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    // The session's explicit hue pins the baseline.
    screen
        .set_application_theme_config(yinz_config::screen_config::ApplicationThemeConfig {
            explicit_theme_hue: Some(yinz_types::style::ThemeHue::Light),
            ..static_application_theme(None)
        })
        .unwrap();
    // A client's own contrary hue only restyles chrome.
    screen
        .apply_configured_explicit_theme_hue(Some(yinz_types::style::ThemeHue::Dark))
        .expect("explicit hue applied");
    // A contrary host report is recorded but must not leak into the answer,
    // and neither may a guess from the palette's (dark) background.
    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Dark)
        .expect("ambient report absorbed");
    screen.core.style.colors.text_unselected.background =
        yinz_types::colors::PaletteColor::Rgb((0, 0, 0));
    let _ = capture.drain_pty_writes();

    let _ = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(3)),
        HostQuery::ColorPaletteMode,
    );

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "exactly one pty reply expected");
    assert_eq!(
        writes[0],
        (b"\x1b[?997;2n".to_vec(), TerminalId::new(3)),
        "a pinned session tells apps the pinned hue, not the host's or a guess"
    );
}

#[test]
fn picker_theme_reaches_plugins_new_panes_and_clients() {
    use yinz_types::keys::{BareKey, KeyWithModifier};
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    add_pane_to_tab(&mut screen, TabId::new(0), PaneId::Plugin(PLUGIN_77), false);
    let original = screen.core.style.colors;
    let preview = styling_with_background(TEST_LIGHT_BG);
    screen.core.themes.insert("preview".into(), preview);
    let terminal = yinz_types::style::TerminalTheme {
        foreground: (1, 2, 3),
        background: (4, 5, 6),
        ansi: [(7, 8, 9); 16],
    };
    screen
        .core
        .terminal_themes
        .insert("preview".into(), terminal);
    screen.toggle_command_palette(CLIENT_1);
    screen.command_palette_handles_input(&None, b"Change theme", false, CLIENT_1);
    capture.drain_plugin_updates();
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Enter)),
        &[],
        false,
        CLIENT_1,
    );
    assert!(capture.drain_plugin_updates().iter().any(
        |(_, _, event)| matches!(event, Event::ModeUpdate(info) if info.style.colors == preview)
    ));
    assert_eq!(screen.core.tabs[&TabId::new(0)].style.colors, preview);
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    assert_eq!(screen.core.default_mode_info.style.colors, preview);
    // A background tab must inherit the preview without navigating away from the picker.
    screen
        .new_tab(TabId::new(1), (vec![], vec![]), None, Some(other))
        .unwrap();
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(2), None)],
            vec![],
            HashMap::new(),
            TabId::new(1),
            false,
            other,
            None,
        )
        .unwrap();
    assert_eq!(screen.core.tabs[&TabId::new(1)].style.colors, preview);
    // The preview renders its terminal block, but applications keep the baseline.
    assert_eq!(
        screen.core.tabs[&TabId::new(1)].style.terminal,
        Some(terminal)
    );
    assert_eq!(
        terminal_background_reply(&mut screen, TabId::new(1), 2),
        vec![b"\x1b]11;rgb:0000/0000/0000\x07".to_vec()]
    );
    screen.close_command_palette(CLIENT_1);
    assert_eq!(screen.core.tabs[&TabId::new(0)].style.colors, original);
    assert_eq!(screen.core.tabs[&TabId::new(1)].style.colors, original);
    assert_eq!(screen.core.tabs[&TabId::new(1)].style.terminal, None);
    assert_eq!(
        terminal_background_reply(&mut screen, TabId::new(1), 2),
        vec![b"\x1b]11;rgb:0000/0000/0000\x07".to_vec()]
    );
    assert_eq!(screen.core.mode_info[&other].style.colors, original);
}

fn terminal_background_reply(screen: &mut Screen, tab: TabId, id: u32) -> Vec<Vec<u8>> {
    let pane = screen
        .core
        .tabs
        .get_mut(&tab)
        .unwrap()
        .get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(id)))
        .unwrap();
    pane.terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b]11;?\x07".to_vec());
    pane.terminal_mut().unwrap().drain_messages_to_pty()
}

#[test]
fn terminal_themes_follow_reload_switching_new_floating_and_hidden_panes() {
    use yinz_types::style::{HostTerminalThemeMode, TerminalTheme};
    let (mut screen, _capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 24 });
    let tab = TabId::new(0);
    new_tab(&mut screen, 1, tab);
    add_pane_to_tab(
        &mut screen,
        tab,
        PaneId::Terminal(TerminalId::new(10)),
        false,
    );
    add_pane_to_tab(
        &mut screen,
        tab,
        PaneId::Terminal(TerminalId::new(11)),
        true,
    );
    screen
        .core
        .tabs
        .get_mut(&tab)
        .unwrap()
        .suppress_pane(PaneId::Terminal(TerminalId::new(10)), Some(CLIENT_1));
    let dark = TerminalTheme {
        foreground: (1, 2, 3),
        background: (4, 5, 6),
        ansi: [(7, 8, 9); 16],
    };
    let light = TerminalTheme {
        background: (240, 240, 240),
        ..dark
    };
    screen
        .set_application_theme_config(static_application_theme(Some(dark)))
        .unwrap();
    // A per-client reload with another terminal block changes neither the render
    // palette nor the answers.
    let mut config = test_screen_config();
    config.terminal_theme = Some(light);
    screen
        .reconfigure(config.clone(), Keybinds::default(), CLIENT_1)
        .unwrap();
    screen
        .change_mode(InputMode::Locked, None, CLIENT_1)
        .unwrap();
    assert_eq!(screen.core.style.terminal, Some(dark));
    screen
        .change_mode(InputMode::Normal, None, CLIENT_1)
        .unwrap();

    for id in [10, 11] {
        assert_eq!(
            terminal_background_reply(&mut screen, tab, id),
            vec![b"\x1b]11;rgb:0404/0505/0606\x07".to_vec()]
        );
    }
    add_pane_to_tab(
        &mut screen,
        tab,
        PaneId::Terminal(TerminalId::new(12)),
        false,
    );
    assert_eq!(
        terminal_background_reply(&mut screen, tab, 12),
        vec![b"\x1b]11;rgb:0404/0505/0606\x07".to_vec()]
    );
    // Viewer host reports restyle chrome only.
    screen.core.host_theme_dark_styling = Some(Styling::default());
    screen.core.host_theme_light_styling = Some(Styling::default());
    screen.core.theme_dark_name = Some("dark".into());
    screen.core.theme_light_name = Some("light".into());
    screen.core.terminal_themes = BTreeMap::from([("dark".into(), dark), ("light".into(), light)]);
    screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Light)
        .unwrap();
    for id in [10, 11, 12] {
        assert_eq!(
            terminal_background_reply(&mut screen, tab, id),
            vec![b"\x1b]11;rgb:0404/0505/0606\x07".to_vec()]
        );
    }
    assert_eq!(screen.core.style.terminal, Some(dark));
    // A session-wide pair change switches answers and the rendered palette everywhere,
    // including hidden and newly revealed panes.
    screen
        .set_application_theme_config(yinz_config::screen_config::ApplicationThemeConfig {
            explicit_theme_hue: Some(yinz_types::style::ThemeHue::Light),
            ..paired_application_theme(Some(dark), Some(light))
        })
        .unwrap();
    assert_eq!(screen.core.style.terminal, Some(light));
    for id in [10, 11, 12] {
        assert_eq!(
            terminal_background_reply(&mut screen, tab, id),
            vec![b"\x1b]11;rgb:f0f0/f0f0/f0f0\x07".to_vec()]
        );
        let pane = screen
            .core
            .tabs
            .get_mut(&tab)
            .unwrap()
            .get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(id)))
            .unwrap();
        pane.terminal_mut()
            .unwrap()
            .handle_pty_bytes(b"content".to_vec());
        let (chunks, _, _, _) = pane.render(Some(CLIENT_1)).unwrap().unwrap();
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|chunk| chunk.pane_default_bg
            == Some(yinz_terminal::panes::terminal_character::AnsiCode::RgbCode(
                light.background
            ))));
    }
    screen
        .core
        .tabs
        .get_mut(&tab)
        .unwrap()
        .unsuppress_pane(PaneId::Terminal(TerminalId::new(10)), false);
    assert_eq!(
        terminal_background_reply(&mut screen, tab, 10),
        vec![b"\x1b]11;rgb:f0f0/f0f0/f0f0\x07".to_vec()]
    );
    // Without a terminal block, applications get the documented fallbacks.
    screen
        .set_application_theme_config(static_application_theme(None))
        .unwrap();
    assert_eq!(screen.core.style.terminal, None);
    for id in [10, 11, 12] {
        assert_eq!(
            terminal_background_reply(&mut screen, tab, id),
            vec![b"\x1b]11;rgb:0000/0000/0000\x07".to_vec()]
        );
    }
}

#[test]
fn attachment_generation_late_theme_report_cannot_be_remembered_while_pinned() {
    use crate::screen::thread_main::{process_instruction, EventLoopState};
    use yinz_types::{
        render_negotiation::AttachmentGeneration,
        style::{HostTerminalThemeMode as Mode, ThemeHue},
    };
    let (mut screen, capture) =
        create_new_screen_with_dark_and_light_themes(Size { cols: 80, rows: 20 });
    let old = ClientId::for_attachment(2, AttachmentGeneration([1; 16]));
    let new = ClientId::for_attachment(2, AttachmentGeneration([2; 16]));
    let owners = screen.core.bus.senders.session.clone();
    owners.register_attachment(CLIENT_1);
    owners.register_attachment(old);
    let held = ScreenInstruction::HostTerminalThemeChanged(old, Mode::Light);
    owners.register_attachment(new);
    let mut state = EventLoopState::default();
    for pinned in [false, true] {
        if pinned {
            screen
                .apply_configured_explicit_theme_hue(Some(ThemeHue::Light))
                .unwrap();
        }
        capture.drain_plugin_events();
        for op in [
            ScreenInstruction::HostTerminalThemeChanged(new, Mode::Dark),
            held.clone(),
        ] {
            assert!(process_instruction(&mut screen, &mut state, op)
                .unwrap()
                .is_continue());
        }
        let events: Vec<_> = capture
            .drain_plugin_events()
            .into_iter()
            .filter_map(|e| match e {
                Event::HostTerminalThemeChanged(mode) => Some(mode),
                _ => None,
            })
            .collect();
        assert_eq!(events, if pinned { vec![] } else { vec![Mode::Dark] });
        assert_eq!(screen.core.last_host_reported_theme_mode, Some(Mode::Dark));
        assert_eq!(
            screen.core.host_terminal_theme_mode,
            Some(if pinned { Mode::Light } else { Mode::Dark })
        );
    }
    screen.apply_configured_explicit_theme_hue(None).unwrap();
    assert_eq!(
        screen.core.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb(TEST_DARK_BG)
    );
    assert!(process_instruction(
        &mut screen,
        &mut state,
        ScreenInstruction::HostTerminalThemeChanged(CLIENT_1, Mode::Light)
    )
    .unwrap()
    .is_continue());
    assert_eq!(
        screen.core.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb(TEST_LIGHT_BG)
    );
}

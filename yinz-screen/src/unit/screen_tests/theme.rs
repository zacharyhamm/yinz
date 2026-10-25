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
        screen.host_terminal_theme_mode,
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
    screen.host_terminal_theme_mode = Some(yinz_types::style::HostTerminalThemeMode::Dark);

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
        screen.forward_in_flight_token.is_none(),
        "slot must remain free; short-circuit does not occupy the queue"
    );
}

#[test]
fn color_palette_mode_query_short_circuits_to_light_reply() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(yinz_types::style::HostTerminalThemeMode::Light);

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
        screen.host_terminal_theme_mode.is_none(),
        "precondition: no host mode learned yet"
    );
    screen.style.colors.text_unselected.background =
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
fn color_palette_mode_query_falls_back_to_own_light_theme_when_host_mode_unknown() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    assert!(
        screen.host_terminal_theme_mode.is_none(),
        "precondition: no host mode learned yet"
    );
    screen.style.colors.text_unselected.background =
        yinz_types::colors::PaletteColor::Rgb((255, 255, 255));

    let _ = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(7)),
        HostQuery::ColorPaletteMode,
    );

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (b"\x1b[?997;2n".to_vec(), TerminalId::new(7)));
}

#[test]
fn color_palette_mode_query_prefers_known_host_mode_over_own_theme() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(yinz_types::style::HostTerminalThemeMode::Light);
    screen.style.colors.text_unselected.background =
        yinz_types::colors::PaletteColor::Rgb((0, 0, 0));

    let _ = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(9)),
        HostQuery::ColorPaletteMode,
    );

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0],
        (b"\x1b[?997;2n".to_vec(), TerminalId::new(9)),
        "the host's announced mode must win over the local theme fallback"
    );
}

#[test]
fn color_palette_mode_query_skips_plugin_panes() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(yinz_types::style::HostTerminalThemeMode::Dark);

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
fn host_theme_no_pty_writes_when_no_panes_subscribed() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);

    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Dark)
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert!(
        writes.is_empty(),
        "no panes exist (or none subscribed via CSI ?2031h), so no DSR forward is queued, got: {:?}",
        writes
    );
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
    screen.host_theme_dark_styling = Some(styling_with_background(TEST_DARK_BG));
    screen.host_theme_light_styling = Some(styling_with_background(TEST_LIGHT_BG));
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
        screen.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Light),
        "an explicit hue must become the session's effective mode"
    );
    assert_eq!(
        screen.style.colors.text_unselected.background,
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
        screen.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Dark),
    );
    assert_eq!(
        screen.style.colors.text_unselected.background,
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
        screen.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Light),
        "the default is not a pin - the host terminal remains authoritative"
    );
    assert_eq!(
        screen.style.colors.text_unselected.background,
        yinz_types::colors::PaletteColor::Rgb(TEST_LIGHT_BG),
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
        .apply_configured_explicit_theme_hue(Some(yinz_types::style::ThemeHue::Light))
        .expect("explicit hue applied");

    screen
        .resolve_default_theme_mode()
        .expect("default is a no-op here");

    assert_eq!(
        screen.host_terminal_theme_mode,
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
        screen.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Light),
        "the pinned mode must survive an ambient report to the contrary"
    );
    assert_eq!(
        screen.style.colors.text_unselected.background,
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
        screen.host_terminal_theme_mode,
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
        screen.host_terminal_theme_mode,
        Some(yinz_types::style::HostTerminalThemeMode::Dark),
        "unpinning must restore the ambient report that was suppressed"
    );
    assert_eq!(
        screen.style.colors.text_unselected.background,
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
    screen.style.colors = styling_with_background((1, 2, 3));

    screen
        .apply_configured_explicit_theme_hue(Some(yinz_types::style::ThemeHue::Dark))
        .expect("explicit hue applied");

    assert_eq!(
        screen.style.colors.text_unselected.background,
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
    screen.style.colors = styling_with_background((1, 2, 3));

    screen
        .apply_configured_explicit_theme_hue(None)
        .expect("unpinned");

    assert_eq!(
        screen.style.colors.text_unselected.background,
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
    screen.host_theme_light_styling = Some(new_light);
    screen.style.colors = styling_with_background((1, 2, 3));

    screen
        .reapply_effective_theme_mode()
        .expect("mode reasserted");

    assert_eq!(
        screen.style.colors.text_unselected.background,
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
    screen
        .apply_configured_explicit_theme_hue(Some(yinz_types::style::ThemeHue::Light))
        .expect("explicit hue applied");
    // A contrary host report is recorded but must not leak into the answer,
    // and neither may a guess from the palette's (dark) background.
    screen
        .update_host_terminal_theme_mode(yinz_types::style::HostTerminalThemeMode::Dark)
        .expect("ambient report absorbed");
    screen.style.colors.text_unselected.background =
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
    let original = screen.style.colors;
    let preview = styling_with_background(TEST_LIGHT_BG);
    screen.themes.insert("preview".into(), preview);
    let terminal = yinz_types::style::TerminalTheme {
        foreground: (1, 2, 3),
        background: (4, 5, 6),
        ansi: [(7, 8, 9); 16],
    };
    screen.terminal_themes.insert("preview".into(), terminal);
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
    assert_eq!(screen.tabs[&TabId::new(0)].style.colors, preview);
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    assert_eq!(screen.default_mode_info.style.colors, preview);
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
    assert_eq!(screen.tabs[&TabId::new(1)].style.colors, preview);
    assert_eq!(screen.tabs[&TabId::new(1)].style.terminal, Some(terminal));
    assert_eq!(
        terminal_background_reply(&mut screen, TabId::new(1), 2),
        vec![b"\x1b]11;rgb:0404/0505/0606\x07".to_vec()]
    );
    screen.close_command_palette(CLIENT_1);
    assert_eq!(screen.tabs[&TabId::new(0)].style.colors, original);
    assert_eq!(screen.tabs[&TabId::new(1)].style.colors, original);
    assert!(terminal_background_reply(&mut screen, TabId::new(1), 2).is_empty());
    assert_eq!(screen.mode_info[&other].style.colors, original);
}

fn terminal_background_reply(screen: &mut Screen, tab: TabId, id: u32) -> Vec<Vec<u8>> {
    let pane = screen
        .tabs
        .get_mut(&tab)
        .unwrap()
        .get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(id)))
        .unwrap();
    pane.handle_pty_bytes(b"\x1b]11;?\x07".to_vec());
    pane.terminal_io_mut().unwrap().drain_messages_to_pty()
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
    let mut config = test_screen_config();
    config.terminal_theme = Some(dark);
    screen
        .reconfigure(config.clone(), Keybinds::default(), CLIENT_1)
        .unwrap();
    screen
        .change_mode(InputMode::Locked, None, CLIENT_1)
        .unwrap();
    assert_eq!(screen.style.terminal, Some(dark));
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
    screen.host_theme_dark_styling = Some(Styling::default());
    screen.host_theme_light_styling = Some(Styling::default());
    screen.theme_dark_name = Some("dark".into());
    screen.theme_light_name = Some("light".into());
    screen.terminal_themes = BTreeMap::from([("dark".into(), dark), ("light".into(), light)]);
    screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Light)
        .unwrap();
    for id in [10, 11, 12] {
        assert_eq!(
            terminal_background_reply(&mut screen, tab, id),
            vec![b"\x1b]11;rgb:f0f0/f0f0/f0f0\x07".to_vec()]
        );
    }
    screen
        .tabs
        .get_mut(&tab)
        .unwrap()
        .unsuppress_pane(PaneId::Terminal(TerminalId::new(10)), false);
    assert_eq!(
        terminal_background_reply(&mut screen, tab, 10),
        vec![b"\x1b]11;rgb:f0f0/f0f0/f0f0\x07".to_vec()]
    );
    screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Dark)
        .unwrap();
    assert_eq!(
        terminal_background_reply(&mut screen, tab, 10),
        vec![b"\x1b]11;rgb:0404/0505/0606\x07".to_vec()]
    );
    config.terminal_theme = None;
    screen
        .reconfigure(config, Keybinds::default(), CLIENT_1)
        .unwrap();
    for id in [10, 11, 12] {
        assert!(terminal_background_reply(&mut screen, tab, id).is_empty());
    }
}

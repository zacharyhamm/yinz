//! Shared applications get one server-owned color baseline, whichever viewer is attached.
//! See `docs/TERMINAL_COLORS.md`.

use super::*;
use yinz_config::screen_config::ApplicationThemeConfig;
use yinz_types::keys::{BareKey, KeyWithModifier};
use yinz_types::style::{HostTerminalThemeMode, TerminalTheme, ThemeHue};

const CLIENT: ClientId = ClientId::new(1);
const TAB: TabId = TabId::new(0);
const PANE: u32 = 1;
const QUERIES: &[u8] = b"\x1b]10;?\x07\x1b]11;?\x07\x1b]4;1;?\x07\x1b]4;200;?\x07\x1b[?996n";

struct Fixture {
    screen: Screen,
    capture: ForwardCapture,
    to_screen: Receiver<(ScreenInstruction, ErrorContext)>,
    plugins: Receiver<(PluginInstruction, ErrorContext)>,
}

/// A screen with one tab whose terminal pane opted into `CSI ? 2031 h`.
fn fixture() -> Fixture {
    let (mut screen, capture) = create_new_screen_with_forward_capture(Size { cols: 80, rows: 24 });
    let (to_screen, screen_rx) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    let (to_plugin, plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    new_tab(&mut screen, PANE, TAB);
    let mut fixture = Fixture {
        screen,
        capture,
        to_screen: screen_rx,
        plugins,
    };
    fixture.feed(PANE, b"\x1b[?2031h");
    fixture.drain();
    fixture
}

impl Fixture {
    /// Pane output, with the queries the tab hands to the screen routed as the screen
    /// thread routes them. Color queries must never get that far.
    fn feed(&mut self, id: u32, bytes: &[u8]) {
        let pane = PaneId::Terminal(TerminalId::new(id));
        self.screen
            .tab_containing_pane_mut(pane)
            .unwrap()
            .handle_pty_bytes(TerminalId::new(id), bytes.to_vec())
            .unwrap();
        while let Ok((instruction, _)) = self.to_screen.try_recv() {
            if let ScreenInstruction::ForwardHostQuery { pane_id, query } = instruction {
                assert!(
                    !matches!(
                        query,
                        HostQuery::DefaultForeground { .. }
                            | HostQuery::DefaultBackground { .. }
                            | HostQuery::PaletteRegister { .. }
                    ),
                    "a color query left the pane: {query:?}"
                );
                self.screen.forward_host_query(pane_id, query);
            }
        }
    }

    fn drain(&self) {
        let _ = self.capture.drain_pty_writes();
    }

    /// Everything written to any PTY since the last drain, as text.
    fn writes(&self) -> Vec<(u32, String)> {
        self.capture
            .drain_pty_writes()
            .into_iter()
            .map(|(bytes, id)| (id.get(), String::from_utf8(bytes).unwrap()))
            .collect()
    }

    fn appearance_notifications(&self) -> Vec<(u32, String)> {
        self.writes()
            .into_iter()
            .filter(|(_, write)| write.starts_with("\x1b[?997;"))
            .collect()
    }

    /// OSC 10, OSC 11, OSC 4;1, OSC 4;200 and CSI ?996n answers for pane `id`.
    fn answers(&mut self, id: u32) -> Vec<String> {
        self.drain();
        self.feed(id, QUERIES);
        self.writes()
            .into_iter()
            .filter(|(pane, _)| *pane == id)
            .map(|(_, write)| write)
            .collect()
    }

    fn set_theme(&mut self, config: ApplicationThemeConfig) {
        self.screen.set_application_theme_config(config).unwrap();
    }

    fn open_theme_picker(&mut self) {
        self.screen.toggle_command_palette(CLIENT);
        self.screen
            .command_palette_handles_input(&None, b"Change theme", false, CLIENT);
        self.key(BareKey::Enter);
        assert!(self.screen.theme_preview.is_some());
    }

    fn key(&mut self, key: BareKey) {
        self.screen.command_palette_handles_input(
            &Some(KeyWithModifier::new(key)),
            &[],
            false,
            CLIENT,
        );
    }
}

fn expected(theme: TerminalTheme, mode: HostTerminalThemeMode) -> Vec<String> {
    let body = |(r, g, b): (u8, u8, u8)| {
        format!(
            "rgb:{:04x}/{:04x}/{:04x}",
            r as u16 * 0x101,
            g as u16 * 0x101,
            b as u16 * 0x101
        )
    };
    vec![
        format!("\x1b]10;{}\x07", body(theme.foreground)),
        format!("\x1b]11;{}\x07", body(theme.background)),
        format!("\x1b]4;1;{}\x07", body(theme.ansi[1])),
        format!("\x1b]4;200;{}\x07", body((255, 0, 215))),
        match mode {
            HostTerminalThemeMode::Dark => "\x1b[?997;1n".to_owned(),
            HostTerminalThemeMode::Light => "\x1b[?997;2n".to_owned(),
        },
    ]
}

fn dark() -> TerminalTheme {
    terminal_block(0x20)
}

fn light() -> TerminalTheme {
    TerminalTheme {
        background: (0xf0, 0xf1, 0xf2),
        ..terminal_block(0x60)
    }
}

/// Chrome stylings and terminal blocks for a `night` / `day` pair, with the session
/// theme defining the same pair.
fn with_pair(fixture: &mut Fixture) -> (Styling, Styling) {
    let mut night = Styling::default();
    night.text_unselected.background = yinz_types::colors::PaletteColor::Rgb((17, 17, 17));
    let mut day = Styling::default();
    day.text_unselected.background = yinz_types::colors::PaletteColor::Rgb((238, 238, 238));
    let screen = &mut fixture.screen;
    screen.themes = BTreeMap::from([("night".into(), night), ("day".into(), day)]);
    screen.terminal_themes = BTreeMap::from([("night".into(), dark()), ("day".into(), light())]);
    screen.theme_dark_name = Some("night".into());
    screen.theme_light_name = Some("day".into());
    screen.host_theme_dark_styling = Some(night);
    screen.host_theme_light_styling = Some(day);
    fixture.set_theme(paired_application_theme(Some(dark()), Some(light())));
    fixture.drain();
    (night, day)
}

#[test]
fn application_baseline_ignores_conflicting_viewer_reports() {
    let mut f = fixture();
    f.set_theme(static_application_theme(Some(dark())));
    let baseline = f.answers(PANE);
    assert_eq!(baseline, expected(dark(), HostTerminalThemeMode::Dark));

    // Two viewers report opposite host colors; neither reaches the application.
    for (fg, bg, register) in [
        (
            "rgb:0000/0000/0000",
            "rgb:ffff/ffff/ffff",
            "rgb:1111/1111/1111",
        ),
        (
            "rgb:fefe/fefe/fefe",
            "rgb:0101/0101/0101",
            "rgb:2222/2222/2222",
        ),
    ] {
        f.screen.update_terminal_foreground_color(fg.into());
        f.screen.update_terminal_background_color(bg.into());
        f.screen
            .update_terminal_color_registers(vec![(1, register.into()), (200, register.into())]);
        assert_eq!(f.answers(PANE), baseline);
    }
    // Their appearance reports neither change the answers nor reach the shared PTY.
    for mode in [
        HostTerminalThemeMode::Light,
        HostTerminalThemeMode::Dark,
        HostTerminalThemeMode::Light,
    ] {
        f.screen.update_host_terminal_theme_mode(mode).unwrap();
        assert!(f.appearance_notifications().is_empty());
        assert_eq!(f.answers(PANE), baseline);
    }
    assert!(f.capture.drain_forward_queries().is_empty());
}

#[test]
fn application_baseline_without_terminal_theme_uses_stable_fallbacks() {
    let mut f = fixture();
    f.screen
        .update_terminal_background_color("rgb:ffff/ffff/ffff".into());
    let fallback = yinz_types::style::ApplicationBaseline::fallback(HostTerminalThemeMode::Dark);
    assert_eq!(
        f.answers(PANE),
        expected(fallback.theme, HostTerminalThemeMode::Dark)
    );
    // A light static theme without a terminal block uses the light fallback.
    f.set_theme(ApplicationThemeConfig {
        static_background: yinz_types::colors::PaletteColor::Rgb((250, 250, 250)),
        ..static_application_theme(None)
    });
    let fallback = yinz_types::style::ApplicationBaseline::fallback(HostTerminalThemeMode::Light);
    assert_eq!(
        f.answers(PANE),
        expected(fallback.theme, HostTerminalThemeMode::Light)
    );
    assert_eq!(f.screen.style.terminal, None);
}

#[test]
fn application_baseline_preserves_overrides_and_resets() {
    let mut f = fixture();
    f.set_theme(static_application_theme(Some(dark())));
    let pane = PaneId::Terminal(TerminalId::new(PANE));
    f.screen
        .tab_containing_pane_mut(pane)
        .unwrap()
        .set_pane_color(pane, Some("#112233".into()), Some("#445566".into()))
        .unwrap();
    let answers = f.answers(PANE);
    assert_eq!(answers[0], "\x1b]10;rgb:1111/2222/3333\x07");
    assert_eq!(answers[1], "\x1b]11;rgb:4444/5555/6666\x07");

    f.feed(
        PANE,
        b"\x1b]10;#abcdef\x07\x1b]11;#fedcba\x07\x1b]4;1;#123456\x07",
    );
    let answers = f.answers(PANE);
    assert_eq!(answers[0], "\x1b]10;rgb:abab/cdcd/efef\x07");
    assert_eq!(answers[1], "\x1b]11;rgb:fefe/dcdc/baba\x07");
    assert_eq!(answers[2], "\x1b]4;1;rgb:1212/3434/5656\x07");

    f.feed(PANE, b"\x1b]110\x07\x1b]111\x07\x1b]104\x07");
    f.screen
        .tab_containing_pane_mut(pane)
        .unwrap()
        .set_pane_color(pane, None, None)
        .unwrap();
    assert_eq!(
        f.answers(PANE),
        expected(dark(), HostTerminalThemeMode::Dark)
    );
}

#[test]
fn application_baseline_ignores_theme_preview() {
    let mut f = fixture();
    f.set_theme(static_application_theme(Some(dark())));
    let baseline = f.answers(PANE);
    let preview = light();
    let mut preview_styling = Styling::default();
    preview_styling.text_unselected.background = yinz_types::colors::PaletteColor::Rgb((1, 2, 3));
    f.screen.themes = BTreeMap::from([
        ("preview".into(), preview_styling),
        ("zzz".into(), f.screen.style.colors),
    ]);
    f.screen.terminal_themes = BTreeMap::from([("preview".into(), preview)]);
    f.screen.theme_name = Some("zzz".into());
    f.open_theme_picker();
    f.key(BareKey::Up);
    assert_eq!(f.screen.style.terminal, Some(preview));
    assert_eq!(f.answers(PANE), baseline);
    f.screen.close_command_palette(CLIENT);
    assert_eq!(f.screen.style.terminal, Some(dark()));
    assert_eq!(f.answers(PANE), baseline);

    // A saved theme is a session-wide change and does move the answers.
    f.set_theme(static_application_theme(Some(preview)));
    assert_eq!(
        f.answers(PANE),
        expected(preview, HostTerminalThemeMode::Light)
    );
}

#[test]
fn application_baseline_follows_pins_and_announces_once() {
    let mut f = fixture();
    with_pair(&mut f);
    assert_eq!(
        f.answers(PANE),
        expected(dark(), HostTerminalThemeMode::Dark)
    );
    let mut completion = None;

    f.screen
        .apply_manual_host_terminal_theme_mode(HostTerminalThemeMode::Light, &mut completion)
        .unwrap();
    assert_eq!(
        f.appearance_notifications(),
        vec![(PANE, "\x1b[?997;2n".to_owned())]
    );
    assert_eq!(f.screen.style.terminal, Some(light()));
    assert_eq!(
        f.answers(PANE),
        expected(light(), HostTerminalThemeMode::Light)
    );

    // Repeating the pin, a per-client reload or a host report while pinned says nothing.
    f.screen
        .apply_manual_host_terminal_theme_mode(HostTerminalThemeMode::Light, &mut completion)
        .unwrap();
    f.screen
        .reconfigure(test_screen_config(), Keybinds::default(), CLIENT)
        .unwrap();
    f.screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Dark)
        .unwrap();
    assert!(f.appearance_notifications().is_empty());
    assert_eq!(f.screen.style.terminal, Some(light()));
    assert_eq!(
        f.answers(PANE),
        expected(light(), HostTerminalThemeMode::Light)
    );

    f.screen
        .apply_manual_host_terminal_theme_mode(HostTerminalThemeMode::Dark, &mut completion)
        .unwrap();
    assert_eq!(
        f.appearance_notifications(),
        vec![(PANE, "\x1b[?997;1n".to_owned())]
    );
    assert_eq!(
        f.answers(PANE),
        expected(dark(), HostTerminalThemeMode::Dark)
    );
}

#[test]
fn application_baseline_survives_client_mode_change() {
    let mut f = fixture();
    f.set_theme(static_application_theme(Some(light())));
    let baseline = f.screen.style.application;
    f.screen
        .change_mode(InputMode::Locked, None, CLIENT)
        .unwrap();
    f.screen
        .change_mode(InputMode::Normal, None, CLIENT)
        .unwrap();
    assert_eq!(f.screen.style.application, baseline);
    // A tab created afterwards copies the screen's style.
    new_tab(&mut f.screen, 2, TabId::new(1));
    assert_eq!(
        f.answers(2),
        expected(light(), HostTerminalThemeMode::Light)
    );
}

#[test]
fn application_baseline_answers_nested_guest_queries() {
    // A guest Yinz client's startup queries, written to the host pane's PTY.
    const GUEST: &[u8] = b"\x1b]11;?\x1b\\\x1b]10;?\x1b\\\x1b[?2031h\x1b[?996n";
    let mut f = fixture();
    f.set_theme(static_application_theme(Some(dark())));
    let guest = |f: &mut Fixture| {
        f.drain();
        f.feed(PANE, GUEST);
        f.writes()
            .into_iter()
            .map(|(_, write)| write)
            .collect::<Vec<_>>()
    };
    let expected = vec![
        "\x1b]11;rgb:2323/2424/2525\x1b\\".to_owned(),
        "\x1b]10;rgb:2020/2121/2222\x1b\\".to_owned(),
        "\x1b[?997;1n".to_owned(),
    ];
    assert_eq!(guest(&mut f), expected);
    f.screen
        .update_terminal_background_color("rgb:ffff/ffff/ffff".into());
    f.screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Light)
        .unwrap();
    assert_eq!(guest(&mut f), expected);
}

#[test]
fn application_baseline_reaches_floating_suppressed_and_new_panes() {
    let mut f = fixture();
    add_pane_to_tab(
        &mut f.screen,
        TAB,
        PaneId::Terminal(TerminalId::new(2)),
        true,
    );
    add_pane_to_tab(
        &mut f.screen,
        TAB,
        PaneId::Terminal(TerminalId::new(3)),
        false,
    );
    f.screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .suppress_pane(PaneId::Terminal(TerminalId::new(3)), Some(CLIENT));
    f.set_theme(static_application_theme(Some(light())));
    add_pane_to_tab(
        &mut f.screen,
        TAB,
        PaneId::Terminal(TerminalId::new(4)),
        false,
    );
    for id in [1, 2, 3, 4] {
        let pane = f
            .screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(id)))
            .unwrap();
        pane.handle_pty_bytes(b"\x1b]11;?\x07\x1b]4;1;?\x07".to_vec());
        assert_eq!(
            pane.terminal_io_mut().unwrap().drain_messages_to_pty(),
            vec![
                b"\x1b]11;rgb:f0f0/f1f1/f2f2\x07".to_vec(),
                b"\x1b]4;1;rgb:6060/0101/6666\x07".to_vec(),
            ],
            "pane {id}"
        );
    }
}

#[test]
fn application_baseline_ignores_per_client_reconfigure() {
    let mut f = fixture();
    f.set_theme(static_application_theme(Some(dark())));
    let baseline = f.answers(PANE);
    // A second client's own config: another theme, a pair and an explicit light hue.
    let mut config = test_screen_config();
    config.terminal_theme = Some(light());
    config.theme_dark_name = Some("night".into());
    config.theme_light_name = Some("day".into());
    config.host_theme_dark = Some(Styling::default());
    config.host_theme_light = Some(Styling::default());
    config.explicit_theme_hue = Some(ThemeHue::Light);
    config.application_theme = ApplicationThemeConfig {
        explicit_theme_hue: Some(ThemeHue::Light),
        ..static_application_theme(Some(light()))
    };
    f.screen.add_client(ClientId::new(2)).unwrap();
    f.screen
        .reconfigure(config.clone(), Keybinds::default(), ClientId::new(2))
        .unwrap();
    assert!(f.appearance_notifications().is_empty());
    assert_eq!(f.screen.style.terminal, Some(dark()));
    assert_eq!(f.answers(PANE), baseline);

    // The same theme delivered as the session's theme does change them, once.
    f.set_theme(config.application_theme);
    assert_eq!(
        f.appearance_notifications(),
        vec![(PANE, "\x1b[?997;2n".to_owned())]
    );
    assert_eq!(f.screen.style.terminal, Some(light()));
    assert_eq!(
        f.answers(PANE),
        expected(light(), HostTerminalThemeMode::Light)
    );
}

#[test]
fn application_baseline_keeps_render_palette_on_baseline_under_auto_pair() {
    let mut f = fixture();
    let (_, day) = with_pair(&mut f);
    f.screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Light)
        .unwrap();
    // The viewer's chrome follows its terminal; panes and answers stay on the baseline.
    assert_eq!(f.screen.style.colors, day);
    assert_eq!(f.screen.style.terminal, Some(dark()));
    assert_eq!(f.screen.tabs[&TAB].style.terminal, Some(dark()));
    assert_eq!(
        f.answers(PANE),
        expected(dark(), HostTerminalThemeMode::Dark)
    );
    assert!(f.plugins.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::Update(updates) if updates.iter().any(|(_, _, event)| matches!(
            event,
            Event::HostTerminalThemeChanged(HostTerminalThemeMode::Light)
        ))
    )));
}

#[test]
fn application_baseline_toggle_uses_baseline_mode() {
    let mut f = fixture();
    with_pair(&mut f);
    f.screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Light)
        .unwrap();
    f.drain();
    let mut completion = None;
    f.screen
        .toggle_manual_host_terminal_theme_mode(&mut completion)
        .unwrap();
    assert_eq!(
        f.appearance_notifications(),
        vec![(PANE, "\x1b[?997;2n".to_owned())]
    );
    assert_eq!(
        f.answers(PANE),
        expected(light(), HostTerminalThemeMode::Light)
    );
}

/// Preview `alpha` (with a terminal block) from a picker opened on `beta`, then ask the
/// server to save it, returning the save request.
fn preview_and_save_alpha(f: &mut Fixture) -> uuid::Uuid {
    let beta = f.screen.style.colors;
    let mut alpha = beta;
    alpha.text_unselected.background = yinz_types::colors::PaletteColor::Rgb((1, 2, 3));
    f.screen.themes = BTreeMap::from([("alpha".into(), alpha), ("beta".into(), beta)]);
    f.screen.terminal_themes = BTreeMap::from([("alpha".into(), light())]);
    f.screen.theme_name = Some("beta".into());
    f.open_theme_picker();
    f.key(BareKey::Up);
    assert_eq!(f.screen.style.terminal, Some(light()));
    f.screen.save_palette_theme(CLIENT, "alpha".into(), false);
    f.capture
        .server_rx
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            ServerInstruction::SavePaletteTheme { request, name, .. } => {
                assert_eq!(name, "alpha");
                Some(request)
            },
            _ => None,
        })
        .unwrap()
}

#[test]
fn application_baseline_theme_save_restores_render_to_baseline() {
    // The server publishes the saved theme's baseline before acknowledging the save.
    let mut f = fixture();
    f.set_theme(static_application_theme(Some(dark())));
    let request = preview_and_save_alpha(&mut f);
    f.set_theme(static_application_theme(Some(light())));
    assert_eq!(f.screen.style.terminal, Some(light()));
    f.screen.palette_theme_saved(CLIENT, request, Ok(()));
    assert!(f.screen.theme_preview.is_none());
    assert_eq!(f.screen.style.terminal, Some(light()));
    assert_eq!(f.screen.tabs[&TAB].style.terminal, Some(light()));
    assert_eq!(
        f.answers(PANE),
        expected(light(), HostTerminalThemeMode::Light)
    );

    // A save another configuration layer overrode leaves the baseline unchanged, and
    // panes return to it instead of keeping the preview.
    let mut f = fixture();
    f.set_theme(static_application_theme(Some(dark())));
    let request = preview_and_save_alpha(&mut f);
    f.screen.palette_theme_saved(CLIENT, request, Ok(()));
    assert!(f.screen.theme_preview.is_none());
    assert_eq!(f.screen.style.terminal, Some(dark()));
    assert_eq!(f.screen.tabs[&TAB].style.terminal, Some(dark()));
    assert_eq!(
        f.answers(PANE),
        expected(dark(), HostTerminalThemeMode::Dark)
    );
}

#[test]
fn application_baseline_preview_normal_entry_keeps_baseline_terminal() {
    let mut f = fixture();
    let (_, day) = with_pair(&mut f);
    f.screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Light)
        .unwrap();
    f.open_theme_picker();
    // A filter that matches no theme previews the viewer's own chrome.
    f.screen
        .command_palette_handles_input(&None, b"no such theme", false, CLIENT);
    assert!(f.screen.theme_preview.is_some());
    assert_eq!(f.screen.style.colors, day);
    assert_eq!(f.screen.style.terminal, Some(dark()));
}

#[test]
fn application_baseline_explicit_light_hue_without_terminal_block_answers_light_fallback() {
    // The documented remedy for a light host terminal without a terminal block
    // (docs/TERMINAL_COLORS.md): the session's own `explicit_theme_hue "light"`.
    use yinz_types::style::ApplicationBaseline;
    let mut f = fixture();
    assert_eq!(
        f.answers(PANE),
        expected(
            ApplicationBaseline::fallback(HostTerminalThemeMode::Dark).theme,
            HostTerminalThemeMode::Dark
        )
    );
    f.set_theme(ApplicationThemeConfig {
        explicit_theme_hue: Some(ThemeHue::Light),
        ..static_application_theme(None)
    });
    assert_eq!(
        f.appearance_notifications(),
        vec![(PANE, "\x1b[?997;2n".to_owned())]
    );
    let light = expected(
        ApplicationBaseline::fallback(HostTerminalThemeMode::Light).theme,
        HostTerminalThemeMode::Light,
    );
    assert_eq!(f.answers(PANE), light);
    // Panes keep rendering host defaults: there is no block to paint with.
    assert_eq!(f.screen.style.terminal, None);

    // A viewer's dark reports change neither the answers nor the PTY.
    f.screen
        .update_terminal_background_color("rgb:0101/0101/0101".into());
    f.screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Dark)
        .unwrap();
    assert!(f.appearance_notifications().is_empty());
    assert_eq!(f.answers(PANE), light);
}

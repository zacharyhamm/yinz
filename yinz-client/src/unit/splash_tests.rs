use super::*;
use crate::test_support::FakeClientOsApi;
use yinz_config::theme::Theme;
use yinz_types::style::{StyleDeclaration, TerminalTheme};

#[test]
fn disabling_the_splash_skips_output_and_the_deadline() {
    for disabled in [None, Some(false), Some(true)] {
        let options = Options {
            disable_splash_screen: disabled,
            ..Default::default()
        };
        let os = FakeClientOsApi::new().capturing_stdout();
        let splash = show(
            &os,
            Size { rows: 24, cols: 80 },
            &Config::default(),
            &options,
        )
        .unwrap();
        assert_eq!(splash.is_none(), disabled == Some(true));
        assert_eq!(os.stdout_bytes().is_empty(), disabled == Some(true));
    }
}

#[test]
fn a_one_cell_margin_is_required_and_small_terminals_have_no_splash() {
    for size in [Size { rows: 9, cols: 80 }, Size { rows: 24, cols: 27 }] {
        let os = FakeClientOsApi::new().capturing_stdout();
        assert!(show(&os, size, &Config::default(), &Options::default())
            .unwrap()
            .is_none());
        assert!(os.stdout_bytes().is_empty());
    }
    let stage = renderer::Stage::new(Size { rows: 10, cols: 28 }).unwrap();
    assert_eq!((stage.cols, stage.rows), (26, 8));
    assert!(renderer::Renderer::new(
        stage,
        StyleDeclaration::default(),
        PaletteColor::EightBit(2),
    )
    .frame(Duration::ZERO)
    .contains("\x1b[2;2H"));
}

#[test]
fn startup_theme_lookup_and_palette_fallback_are_used() {
    let mut config = Config::from_default_assets().unwrap();
    config.themes = yinz_config::assets::get_default_themes().merge(config.themes);
    let size = Size { rows: 24, cols: 80 };
    for theme in [
        None,
        Some("dracula"),
        Some("ayu-light"),
        Some("ansi"),
        Some("missing-theme"),
    ] {
        let options = Options {
            theme: theme.map(str::to_owned),
            ..Default::default()
        };
        let os = FakeClientOsApi::new().capturing_stdout();
        let before = Instant::now();
        let mut splash = show(&os, size, &config, &options).unwrap().unwrap();
        assert_eq!(
            splash.deadline() - splash.started,
            Duration::from_millis(1000)
        );
        assert!(splash.deadline() >= before + DURATION);
        assert!(splash.deadline() <= Instant::now() + DURATION);
        let theme = config
            .theme_config(options.theme.as_ref())
            .unwrap_or_else(|| default_palette().into());
        if matches!(options.theme.as_deref(), Some("dracula" | "ayu-light")) {
            assert!(config.theme_config(options.theme.as_ref()).is_some());
        }
        let mut style = theme.text_unselected;
        if let Some(terminal) = config.terminal_theme_config(options.theme.as_ref()) {
            style.base = PaletteColor::Rgb(terminal.foreground);
            style.background = PaletteColor::Rgb(terminal.background);
        }
        let mut renderer = renderer::Renderer::new(
            renderer::Stage::new(size).unwrap(),
            style,
            theme.frame_selected.base,
        );
        assert_eq!(os.stdout_bytes(), renderer.frame(Duration::ZERO).as_bytes());
        let elapsed = Duration::from_millis(500);
        let before = os.stdout_bytes().len();
        splash.tick(&os, None, splash.started + elapsed).unwrap();
        assert_eq!(
            &os.stdout_bytes()[before..],
            renderer.frame(elapsed).as_bytes()
        );
    }
}

#[test]
fn shared_color_code_supports_indexed_colors() {
    assert_eq!(
        crate::terminal_escapes::color_code(PaletteColor::EightBit(255)),
        "5;255"
    );
}

#[test]
fn selected_frame_accent_is_used_with_terminal_overrides_and_ui_fallbacks() {
    let size = Size { rows: 24, cols: 80 };
    let ui = StyleDeclaration {
        base: PaletteColor::Rgb((225, 240, 200)),
        background: PaletteColor::Rgb((18, 25, 32)),
        ..Default::default()
    };
    let indexed_ui = StyleDeclaration {
        base: PaletteColor::EightBit(229),
        background: PaletteColor::EightBit(236),
        ..Default::default()
    };
    let terminal = TerminalTheme {
        foreground: (55, 80, 30),
        background: (245, 235, 230),
        ansi: [(0, 0, 0); 16],
    };
    for (terminal, ui, accent) in [
        (Some(terminal), ui, PaletteColor::Rgb((80, 250, 123))),
        (None, ui, PaletteColor::Rgb((185, 110, 230))),
        (Some(terminal), indexed_ui, PaletteColor::EightBit(201)),
        (None, indexed_ui, PaletteColor::EightBit(39)),
    ] {
        for theme_name in [None, Some("custom")] {
            let mut config = Config::default();
            let mut palette = yinz_types::style::Styling::from(default_palette());
            palette.text_unselected = ui;
            palette.frame_selected.base = accent;
            config.themes.insert(
                theme_name.unwrap_or("default").into(),
                Theme {
                    terminal,
                    palette,
                    sourced_from_external_file: false,
                },
            );
            let expected = terminal.map_or(ui, |terminal| StyleDeclaration {
                base: PaletteColor::Rgb(terminal.foreground),
                background: PaletteColor::Rgb(terminal.background),
                ..ui
            });
            let options = Options {
                theme: theme_name.map(str::to_owned),
                ..Default::default()
            };
            let os = FakeClientOsApi::new().capturing_stdout();
            let mut splash = show(&os, size, &config, &options).unwrap().unwrap();
            let mut renderer =
                renderer::Renderer::new(renderer::Stage::new(size).unwrap(), expected, accent);
            assert_eq!(os.stdout_bytes(), renderer.frame(Duration::ZERO).as_bytes());
            let elapsed = Duration::from_millis(500);
            let expected_shimmer = renderer.frame(elapsed);
            let before = os.stdout_bytes().len();
            splash.tick(&os, None, splash.started + elapsed).unwrap();
            assert_eq!(&os.stdout_bytes()[before..], expected_shimmer.as_bytes());
            let mut other_accent = renderer::Renderer::new(
                renderer::Stage::new(size).unwrap(),
                expected,
                PaletteColor::Rgb((255, 160, 0)),
            );
            assert_ne!(expected_shimmer, other_accent.frame(elapsed));
        }
    }
}

#[test]
fn ticks_skip_missed_frames_and_wake_at_the_deadline() {
    let os = FakeClientOsApi::new().capturing_stdout();
    let mut splash = test_splash(Instant::now());
    let late = splash.started + Duration::from_millis(775);
    splash.tick(&os, Some(SyncOutput::Csi), late).unwrap();
    let bytes = os.stdout_bytes();
    assert!(bytes.starts_with(SyncOutput::Csi.start_seq()));
    assert!(bytes.ends_with(SyncOutput::Csi.end_seq()));
    assert_eq!(splash.next_frame, Some(late + FRAME_INTERVAL));
    splash
        .tick(&os, None, late + Duration::from_millis(1))
        .unwrap();
    assert_eq!(bytes, os.stdout_bytes());
    assert!(splash
        .tick(&os, None, splash.deadline() - Duration::from_millis(1))
        .unwrap());
    assert_eq!(splash.wake_at(), splash.deadline());
}

#[test]
fn expiry_clears_once_even_when_the_wakeup_is_late_or_the_terminal_resizes() {
    for delay in [Duration::ZERO, Duration::from_secs(1)] {
        let os = FakeClientOsApi::new().capturing_stdout();
        let mut splash = test_splash(Instant::now());
        assert!(!splash
            .tick(&os, Some(SyncOutput::Csi), splash.deadline() + delay)
            .unwrap());
        let expected = [
            SyncOutput::Csi.start_seq(),
            CLEAR.as_bytes(),
            SyncOutput::Csi.end_seq(),
        ]
        .concat();
        assert_eq!(os.stdout_bytes(), expected);
        assert!(splash.next_frame.is_none());
        let resized = os.clone().with_terminal_size(Size { rows: 14, cols: 41 });
        assert!(!splash
            .tick(&resized, None, splash.deadline() + Duration::from_secs(2))
            .unwrap());
        assert_eq!(os.stdout_bytes(), expected);
    }
}

#[test]
fn resizing_repaints_and_a_too_small_terminal_ends_the_splash() {
    let os = FakeClientOsApi::new().capturing_stdout();
    let mut splash = test_splash(Instant::now());
    let smaller = os.clone().with_terminal_size(Size { rows: 14, cols: 41 });
    assert!(splash.tick(&smaller, None, splash.started).unwrap());
    assert_eq!(
        (splash.renderer.stage.cols, splash.renderer.stage.rows),
        (39, 12)
    );
    assert!(!os.stdout_bytes().is_empty());
    let tiny = os.clone().with_terminal_size(Size { rows: 5, cols: 10 });
    assert!(!splash.tick(&tiny, None, splash.started).unwrap());
    assert!(os.stdout_bytes().ends_with(CLEAR.as_bytes()));
}

#[test]
fn splash_write_errors_are_reported() {
    let os = FakeClientOsApi::new().with_stdout_error(std::io::ErrorKind::BrokenPipe);
    assert!(show(
        &os,
        Size { rows: 24, cols: 80 },
        &Config::default(),
        &Options::default()
    )
    .err()
    .unwrap()
    .to_string()
    .contains("draw the startup splash"));
    let mut splash = test_splash(Instant::now());
    assert!(splash
        .tick(&os, None, splash.started + FRAME_INTERVAL)
        .unwrap_err()
        .to_string()
        .contains("draw the startup splash"));
    assert!(splash
        .tick(&os, None, splash.deadline())
        .unwrap_err()
        .to_string()
        .contains("clear the startup splash"));
}

#[test]
fn eight_bit_theme_colors_are_resolved_for_shading() {
    let stage = renderer::Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    let style = StyleDeclaration {
        background: PaletteColor::EightBit(16),
        base: PaletteColor::EightBit(255),
        ..Default::default()
    };
    assert!(
        renderer::Renderer::new(stage, style, PaletteColor::EightBit(2))
            .frame(Duration::ZERO)
            .starts_with("\x1b[m\x1b[48;2;0;0;0m\x1b[2J")
    );
}

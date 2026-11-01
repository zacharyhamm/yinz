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
    assert!(renderer::Renderer::new(stage, StyleDeclaration::default())
        .frame(Duration::ZERO)
        .contains("\x1b[2;2H"));
}

#[test]
fn startup_theme_lookup_and_palette_fallback_are_used() {
    let config = Config::from_default_assets().unwrap();
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
        let splash = show(&os, size, &config, &options).unwrap().unwrap();
        assert!(splash.deadline() >= before + DURATION);
        assert!(splash.deadline() <= Instant::now() + DURATION);
        let mut style = config
            .theme_config(options.theme.as_ref())
            .unwrap_or_else(|| default_palette().into())
            .text_unselected;
        if let Some(terminal) = config.terminal_theme_config(options.theme.as_ref()) {
            style.base = PaletteColor::Rgb(terminal.foreground);
            style.background = PaletteColor::Rgb(terminal.background);
        }
        let mut renderer = renderer::Renderer::new(renderer::Stage::new(size).unwrap(), style);
        assert_eq!(os.stdout_bytes(), renderer.frame(Duration::ZERO).as_bytes());
    }
}

#[test]
fn shared_color_code_supports_indexed_colors() {
    assert_eq!(crate::terminal_escapes::color_code(PaletteColor::EightBit(255)), "5;255");
}

#[test]
fn terminal_colors_take_precedence_and_ui_colors_remain_the_fallback() {
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
    for (terminal, ui) in [(Some(terminal), ui), (None, ui), (None, indexed_ui)] {
        for theme_name in [None, Some("custom")] {
            let mut config = Config::default();
            let mut palette = yinz_types::style::Styling::from(default_palette());
            palette.text_unselected = ui;
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
            show(&os, size, &config, &options).unwrap().unwrap();
            let mut renderer =
                renderer::Renderer::new(renderer::Stage::new(size).unwrap(), expected);
            assert_eq!(os.stdout_bytes(), renderer.frame(Duration::ZERO).as_bytes());
        }
    }
}

#[test]
fn ticks_skip_missed_frames_and_leave_the_final_pose_without_more_writes() {
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
    splash
        .tick(&os, None, splash.deadline() + Duration::from_secs(1))
        .unwrap();
    let bytes = os.stdout_bytes();
    assert!(splash.next_frame.is_none());
    splash
        .tick(&os, None, splash.deadline() + Duration::from_secs(2))
        .unwrap();
    assert_eq!(bytes, os.stdout_bytes());
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
    let mut splash = test_splash(Instant::now() - Duration::from_secs(1));
    assert!(splash
        .tick(&os, None, Instant::now())
        .unwrap_err()
        .to_string()
        .contains("draw the startup splash"));
}

#[test]
fn eight_bit_theme_colors_are_resolved_for_shading() {
    let stage = renderer::Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    let style = StyleDeclaration {
        background: PaletteColor::EightBit(16),
        base: PaletteColor::EightBit(255),
        ..Default::default()
    };
    assert!(renderer::Renderer::new(stage, style)
        .frame(Duration::ZERO)
        .starts_with("\x1b[m\x1b[48;2;0;0;0m\x1b[2J"));
}

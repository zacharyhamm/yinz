use super::*;
use crate::test_support::FakeClientOsApi;

#[test]
fn disabling_the_splash_skips_output_and_the_deadline() {
    for disabled in [None, Some(false), Some(true)] {
        let options = Options {
            disable_splash_screen: disabled,
            ..Default::default()
        };
        let os = FakeClientOsApi::new().capturing_stdout();
        let deadline = show(
            &os,
            Size { rows: 24, cols: 80 },
            &Config::default(),
            &options,
        )
        .unwrap();
        assert_eq!(deadline.is_none(), disabled == Some(true));
        assert_eq!(os.stdout_bytes().is_empty(), disabled == Some(true));
    }
}

#[test]
fn artwork_and_bands_keep_the_blank_row_and_use_cell_coordinates() {
    let style = StyleDeclaration {
        background: PaletteColor::Rgb((1, 2, 3)),
        emphasis_0: PaletteColor::EightBit(10),
        emphasis_1: PaletteColor::Rgb((20, 21, 22)),
        emphasis_2: PaletteColor::EightBit(30),
        emphasis_3: PaletteColor::Rgb((40, 41, 42)),
        ..Default::default()
    };
    let output = render(Size { rows: 25, cols: 81 }, style).unwrap();
    assert_eq!(ART.len(), 11);
    assert_eq!(ART[0], "           ░██░                      ");
    assert_eq!(ART[1], "                                     ");
    assert_eq!(ART.iter().map(|row| row.chars().count()).max(), Some(37));
    assert!(output.starts_with("\x1b[m\x1b[48;2;1;2;3m\x1b[2J"));
    let colors = ["5;10", "2;20;21;22", "5;30", "2;40;41;42"];
    let mut expected = String::new();
    let mut band = 0;
    for (row, text) in ART.iter().enumerate() {
        if row != 1 {
            expected.push_str(&format!("\x1b[38;{}m", colors[band % 4]));
            band += 1;
        }
        expected.push_str(&format!("\x1b[{};23H{text}", 8 + row));
    }
    assert!(output.ends_with(&format!("{expected}{RESET_STYLE}")));
}

#[test]
fn a_one_cell_margin_is_required_and_small_terminals_have_no_deadline() {
    for size in [Size { rows: 12, cols: 80 }, Size { rows: 24, cols: 38 }] {
        let os = FakeClientOsApi::new().capturing_stdout();
        assert!(show(&os, size, &Config::default(), &Options::default())
            .unwrap()
            .is_none());
        assert!(os.stdout_bytes().is_empty());
    }
    assert!(
        render(Size { rows: 13, cols: 39 }, StyleDeclaration::default())
            .unwrap()
            .contains("\x1b[2;2H")
    );
}

#[test]
fn startup_theme_lookup_and_palette_fallback_are_used() {
    let config = Config::from_default_assets().unwrap();
    let size = Size { rows: 24, cols: 80 };
    for theme in [
        None,
        Some("dracula"),
        Some("ayu-light"),
        Some("missing-theme"),
    ] {
        let options = Options {
            theme: theme.map(str::to_owned),
            ..Default::default()
        };
        let os = FakeClientOsApi::new().capturing_stdout();
        let before = Instant::now();
        let deadline = show(&os, size, &config, &options).unwrap().unwrap();
        assert!(deadline >= before + Duration::from_secs(1));
        assert!(deadline <= Instant::now() + Duration::from_secs(1));
        let style = config
            .theme_config(options.theme.as_ref())
            .unwrap_or_else(|| default_palette().into())
            .text_unselected;
        assert_eq!(os.stdout_bytes(), render(size, style).unwrap().as_bytes());
    }
    assert_eq!(color(PaletteColor::EightBit(255)), "5;255");
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
    .unwrap_err()
    .to_string()
    .contains("draw the startup splash"));
}

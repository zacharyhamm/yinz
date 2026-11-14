//! Shared applications get color answers from the server baseline, never from a viewer.

use super::*;
use yinz_types::colors::PaletteColor;
use yinz_types::style::{ApplicationBaseline, HostTerminalThemeMode, TerminalTheme};

fn baseline() -> ApplicationBaseline {
    ApplicationBaseline {
        theme: TerminalTheme {
            foreground: (0x10, 0x20, 0x30),
            background: (0x40, 0x50, 0x60),
            ansi: std::array::from_fn(|i| (i as u8, 0x70, 0x80)),
        },
        mode: HostTerminalThemeMode::Light,
    }
}

fn queries(grid: &mut Grid, parser: &mut vte::Parser) -> Vec<String> {
    grid.pending_messages_to_pty.clear();
    parser.advance(
        grid,
        b"\x1b]10;?\x07\x1b]11;?\x1b\\\x1b]4;1;?\x07\x1b]4;200;?\x07\x1b]4;250;?\x07",
    );
    assert!(grid.pending_forwarded_queries.is_empty());
    std::mem::take(&mut grid.pending_messages_to_pty)
        .into_iter()
        .map(|reply| String::from_utf8(reply).unwrap())
        .collect()
}

#[test]
fn application_baseline_answers_ignore_viewer_host_colors_and_the_render_theme() {
    let host = Rc::new(RefCell::new(Palette::default()));
    let host_codes = Rc::new(RefCell::new(HashMap::new()));
    let mut grid = Grid::new(
        10,
        20,
        host.clone(),
        host_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    );
    let mut parser = vte::Parser::new();
    grid.set_application_baseline(baseline());
    let expected = vec![
        "\x1b]10;rgb:1010/2020/3030\x07".to_owned(),
        "\x1b]11;rgb:4040/5050/6060\x1b\\".to_owned(),
        "\x1b]4;1;rgb:0101/7070/8080\x07".to_owned(),
        "\x1b]4;200;rgb:ffff/0000/d7d7\x07".to_owned(),
        "\x1b]4;250;rgb:bcbc/bcbc/bcbc\x07".to_owned(),
    ];
    assert_eq!(queries(&mut grid, &mut parser), expected);

    // A viewer's host report and a temporary render theme (a preview) change neither.
    host.borrow_mut().bg = PaletteColor::Rgb((1, 1, 1));
    host.borrow_mut().fg = PaletteColor::Rgb((2, 2, 2));
    host_codes
        .borrow_mut()
        .insert(1, "rgb:0909/0909/0909".to_owned());
    grid.update_terminal_theme(Some(TerminalTheme {
        foreground: (9, 9, 9),
        background: (8, 8, 8),
        ansi: [(7, 7, 7); 16],
    }));
    assert_eq!(queries(&mut grid, &mut parser), expected);

    // Only a new baseline changes them.
    grid.set_application_baseline(ApplicationBaseline::fallback(HostTerminalThemeMode::Dark));
    assert_eq!(
        queries(&mut grid, &mut parser),
        vec![
            "\x1b]10;rgb:e5e5/e5e5/e5e5\x07",
            "\x1b]11;rgb:0000/0000/0000\x1b\\",
            "\x1b]4;1;rgb:cdcd/0000/0000\x07",
            "\x1b]4;200;rgb:ffff/0000/d7d7\x07",
            "\x1b]4;250;rgb:bcbc/bcbc/bcbc\x07",
        ]
    );
}

#[test]
fn application_baseline_preserves_overrides_and_resets() {
    let mut grid = new_grid_for_forwarding_test();
    let mut parser = vte::Parser::new();
    grid.set_application_baseline(baseline());

    grid.set_pane_default_colors(Some("#112233".into()), Some("#445566".into()));
    let answers = queries(&mut grid, &mut parser);
    assert_eq!(answers[0], "\x1b]10;rgb:1111/2222/3333\x07");
    assert_eq!(answers[1], "\x1b]11;rgb:4444/5555/6666\x1b\\");

    // Application overrides beat pane defaults.
    parser.advance(
        &mut grid,
        b"\x1b]10;#abcdef\x07\x1b]11;#fedcba\x07\x1b]4;1;#123456;200;#654321\x07",
    );
    let answers = queries(&mut grid, &mut parser);
    assert_eq!(
        answers,
        vec![
            "\x1b]10;rgb:abab/cdcd/efef\x07",
            "\x1b]11;rgb:fefe/dcdc/baba\x1b\\",
            "\x1b]4;1;rgb:1212/3434/5656\x07",
            "\x1b]4;200;rgb:6565/4343/2121\x07",
            "\x1b]4;250;rgb:bcbc/bcbc/bcbc\x07",
        ]
    );

    // Resets return to the pane default, then the baseline.
    parser.advance(&mut grid, b"\x1b]110\x07\x1b]111\x07\x1b]104;1\x07");
    let answers = queries(&mut grid, &mut parser);
    assert_eq!(answers[0], "\x1b]10;rgb:1111/2222/3333\x07");
    assert_eq!(answers[1], "\x1b]11;rgb:4444/5555/6666\x1b\\");
    assert_eq!(answers[2], "\x1b]4;1;rgb:0101/7070/8080\x07");
    assert_eq!(answers[3], "\x1b]4;200;rgb:6565/4343/2121\x07");
    grid.set_pane_default_colors(None, None);
    parser.advance(&mut grid, b"\x1b]104\x07");
    assert_eq!(
        queries(&mut grid, &mut parser),
        vec![
            "\x1b]10;rgb:1010/2020/3030\x07",
            "\x1b]11;rgb:4040/5050/6060\x1b\\",
            "\x1b]4;1;rgb:0101/7070/8080\x07",
            "\x1b]4;200;rgb:ffff/0000/d7d7\x07",
            "\x1b]4;250;rgb:bcbc/bcbc/bcbc\x07",
        ]
    );

    // A full reset also returns to the baseline.
    parser.advance(&mut grid, b"\x1b]10;#abcdef\x07\x1b]4;1;#123456\x07\x1bc");
    let answers = queries(&mut grid, &mut parser);
    assert_eq!(answers[0], "\x1b]10;rgb:1010/2020/3030\x07");
    assert_eq!(answers[2], "\x1b]4;1;rgb:0101/7070/8080\x07");
}

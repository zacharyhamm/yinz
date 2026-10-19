#![cfg(unix)]

use base64::{engine::general_purpose::STANDARD, Engine};
use yinz_integration_tests::{
    AnsiCode, FakePtyHandle, GridSnapshot, LayoutInfo, TestRunner, TERMINAL_SIZE,
};
use yinz_types::{agent::theme::*, input::actions::Action};

fn emit(pane: &FakePtyHandle, provider: &str, role: Role, rgb: (u8, u8, u8), label: &str) {
    let snapshot = AgentThemeSnapshot {
        version: SNAPSHOT_VERSION,
        provider: provider.into(),
        provider_version: "fixture".into(),
        theme: "fixture".into(),
        provenance: vec![],
        availability: Availability::Available,
        definitions: vec![Definition {
            token: "status".into(),
            channel: Channel::Foreground,
            color: SourceColor::Rgb(rgb.0, rgb.1, rgb.2),
            pair: None,
            target: Some(role.into()),
        }],
        collisions: vec![],
    };
    let encoded = STANDARD.encode(
        serde_json::to_vec(&AgentThemeEvent {
            version: EVENT_VERSION,
            launch_id: provider.into(),
            snapshot: Some(snapshot),
        })
        .unwrap(),
    );
    pane.output(
        format!(
            "\x1b]777;yinz-agent-theme;{encoded}\x1b\\\x1b[38;2;{};{};{}m{label}\x1b[0m\r\n",
            rgb.0, rgb.1, rgb.2
        )
        .as_bytes(),
    );
}

fn foreground(grid: &GridSnapshot, label: &str) -> Option<AnsiCode> {
    grid.lines().iter().enumerate().find_map(|(y, line)| {
        line.find(label)
            .and_then(|byte| grid.cell_foreground(line[..byte].chars().count(), y))
    })
}

#[test]
fn simultaneous_source_palettes_follow_theme_changes_and_live_opt_out() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            "load_plugins clear-defaults=true {}\npane_animations false\ntheme \"nightfox\"",
        )
        .with_layout(LayoutInfo::Stringified(
            "layout { pane; pane; pane; }".into(),
        ))
        .start();
    let claude = yinz.expect_pty_spawn();
    let codex = yinz.expect_pty_spawn();
    let ordinary = yinz.expect_pty_spawn();
    emit(
        &claude,
        "claude",
        Role::Success,
        (18, 52, 86),
        "CLAUDE-COLOR",
    );
    emit(&codex, "codex", Role::Error, (9, 8, 7), "CODEX-COLOR");
    ordinary.output(b"\x1b[38;2;18;52;86mRAW-COLOR\x1b[0m\r\n\x1b[38;2;246;226;183mRAW-MODEL\r\n\x1b[38;2;171;223;167mRAW-DIR\r\n\x1b[38;2;133;137;143mRAW-SEP\x1b[0m");
    for (config, success, error) in [
        (None, (129, 178, 154), (201, 79, 109)),
        (Some("theme \"dracula\""), (80, 250, 123), (255, 85, 85)),
        (Some("agent_theme_mapping \"off\""), (18, 52, 86), (9, 8, 7)),
        (
            Some("agent_theme_mapping \"semantic\"\ntheme \"nightfox\""),
            (129, 178, 154),
            (201, 79, 109),
        ),
    ] {
        if let Some(config) = config {
            let (code, error) = yinz.run_actions_reporting(vec![Action::Reconfigure {
                config: config.into(),
                write_to_disk: false,
            }]);
            assert_eq!(code, 0, "{error}");
        }
        yinz.wait_until(
            "each palette and the ordinary pane use their expected colors",
            |grid| {
                foreground(grid, "CLAUDE-COLOR") == Some(AnsiCode::RgbCode(success))
                    && foreground(grid, "CODEX-COLOR") == Some(AnsiCode::RgbCode(error))
                    && foreground(grid, "RAW-COLOR") == Some(AnsiCode::RgbCode((18, 52, 86)))
                    && foreground(grid, "RAW-MODEL") == Some(AnsiCode::RgbCode((246, 226, 183)))
                    && foreground(grid, "RAW-DIR") == Some(AnsiCode::RgbCode((171, 223, 167)))
                    && foreground(grid, "RAW-SEP") == Some(AnsiCode::RgbCode((133, 137, 143)))
            },
        );
    }
    yinz.quit();
}

fn background(grid: &GridSnapshot, label: &str) -> Option<AnsiCode> {
    grid.lines().iter().enumerate().find_map(|(y, line)| {
        line.find(label)
            .and_then(|byte| grid.cell_style(line[..byte].chars().count(), y).background)
    })
}

#[test]
fn claude_file_diff_and_syntax_repaint_on_dark_light_switch_and_opt_out() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            "load_plugins clear-defaults=true {}\npane_animations false\ntheme \"nightfox\"",
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let claude = yinz.expect_pty_spawn();
    claude.disable_echo();
    let snapshot = serde_json::from_str(include_str!(
        "../../yinz-process/src/agent_theme/unit/fixtures/claude-light-code.json"
    ))
    .unwrap();
    let encoded = STANDARD.encode(
        serde_json::to_vec(&AgentThemeEvent {
            version: EVENT_VERSION,
            launch_id: "claude-code".into(),
            snapshot: Some(snapshot),
        })
        .unwrap(),
    );
    claude.output(format!("\x1b]777;yinz-agent-theme;{encoded}\x1b\\").as_bytes());
    claude.output(include_bytes!(
        "../../yinz-process/src/agent_theme/unit/fixtures/claude-light-code.ansi"
    ));

    // Retained output must repaint without another application write.
    for (theme, [fg, accent, success, error, muted], [add, remove, add_word, remove_word]) in [
        (
            "nightfox",
            [
                (205, 206, 207),
                (99, 205, 207),
                (129, 178, 154),
                (201, 79, 109),
                (142, 146, 151),
            ],
            [(44, 61, 67), (57, 43, 59), (56, 78, 80), (78, 48, 66)],
        ),
        (
            "dayfox",
            [
                (61, 43, 90),
                (31, 93, 99),
                (51, 93, 64),
                (162, 33, 46),
                (126, 113, 142),
            ],
            [
                (211, 215, 207),
                (231, 204, 203),
                (188, 197, 186),
                (221, 179, 180),
            ],
        ),
        (
            "nightfox",
            [
                (205, 206, 207),
                (99, 205, 207),
                (129, 178, 154),
                (201, 79, 109),
                (142, 146, 151),
            ],
            [(44, 61, 67), (57, 43, 59), (56, 78, 80), (78, 48, 66)],
        ),
    ] {
        let (code, error_message) = yinz.run_actions_reporting(vec![Action::Reconfigure {
            config: format!("theme \"{theme}\""),
            write_to_disk: false,
        }]);
        assert_eq!(code, 0, "{error_message}");
        yinz.wait_until(
            "retained Claude code and diffs use the selected theme",
            |grid| {
                [
                    ("plain alpha", fg),
                    ("unchanged", fg),
                    ("removed", fg),
                    ("added", fg),
                    ("-removed", error),
                    ("+added", success),
                    ("const", accent),
                    ("identifier", accent),
                    ("\"string\"", success),
                    ("42", accent),
                    ("// comment", muted),
                    ("indexed-string", success),
                    ("indexed-comment", muted),
                ]
                .into_iter()
                .all(|(label, color)| foreground(grid, label) == Some(AnsiCode::RgbCode(color)))
                    && [
                        ("removed", remove),
                        ("old", remove_word),
                        ("added", add),
                        ("new", add_word),
                    ]
                    .into_iter()
                    .all(|(label, color)| background(grid, label) == Some(AnsiCode::RgbCode(color)))
            },
        );
    }
    let (code, error) = yinz.run_actions_reporting(vec![Action::Reconfigure {
        config: "agent_theme_mapping \"off\"".into(),
        write_to_disk: false,
    }]);
    assert_eq!(code, 0, "{error}");
    yinz.wait_until("opt out restores Claude's code palette", |grid| {
        foreground(grid, "plain alpha") == Some(AnsiCode::RgbCode((51, 51, 51)))
            && foreground(grid, "const") == Some(AnsiCode::RgbCode((167, 29, 93)))
            && background(grid, "removed") == Some(AnsiCode::RgbCode((255, 220, 220)))
            && background(grid, "old") == Some(AnsiCode::RgbCode((255, 199, 199)))
            && background(grid, "added") == Some(AnsiCode::RgbCode((220, 255, 220)))
            && background(grid, "new") == Some(AnsiCode::RgbCode((178, 255, 178)))
    });
    yinz.quit();
}

#[test]
fn codex_composer_and_footer_repaint_on_theme_switch_preview_cancel_and_typing() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\npane_animations false\ntheme \"nightfox\"\nkeybinds { normal { bind \"Ctrl Alt p\" { ToggleCommandPalette; }; }; }")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let codex = yinz.expect_pty_spawn();
    codex.disable_echo();
    let snapshot = AgentThemeSnapshot {
        version: SNAPSHOT_VERSION,
        provider: "codex".into(),
        provider_version: "0.162.1".into(),
        theme: "automatic".into(),
        provenance: vec![],
        availability: Availability::Available,
        definitions: [("model", (246, 226, 183)), ("path", (171, 223, 167))]
            .into_iter()
            .map(|(item, (r, g, b))| Definition {
                token: format!("codex.footer.dark.{item}"),
                channel: Channel::Foreground,
                color: SourceColor::Rgb(r, g, b),
                pair: None,
                target: None,
            })
            .collect(),
        collisions: vec![],
    };
    let encoded = STANDARD.encode(
        serde_json::to_vec(&AgentThemeEvent {
            version: EVENT_VERSION,
            launch_id: "live-codex".into(),
            snapshot: Some(snapshot),
        })
        .unwrap(),
    );
    let footer = "\x1b[4;1H\x1b[0;38;2;246;226;183mMODEL\x1b[38;2;133;137;143m · \x1b[38;2;171;223;167mDIR\x1b[0m";
    let indexed_footer = "\x1b[5;1H\x1b[0;38;5;223mINDEX\x1b[38;5;245m : \x1b[38;5;151mPATH\x1b[0m";
    let footer_matches = |grid: &GridSnapshot, colors: [(u8, u8, u8); 3]| {
        // Match footer separators with their suffix so palette chrome cannot match first.
        ["MODEL", "DIR", "· DIR", "INDEX", "PATH", ": PATH"]
            .into_iter()
            .zip(colors.into_iter().cycle())
            .all(|(label, rgb)| foreground(grid, label) == Some(AnsiCode::RgbCode(rgb)))
    };
    let night_footer = [(99, 205, 207), (129, 178, 154), (142, 146, 151)];
    let dracula_footer = [(139, 233, 253), (80, 250, 123), (175, 176, 176)];
    let day_footer = [(31, 93, 99), (51, 93, 64), (126, 113, 142)];
    codex.output(format!("\x1b]777;yinz-agent-theme;{encoded}\x1b\\\x1b]10;?\x07\x1b]11;?\x07\x1b[?25l\x1b[48;2;52;61;72mCOMPOSER   \x1b[0m\r\n\x1b[48;2;61;70;81mSUBMITTED\x1b[0m\r\n\x1b[48;2;52;61;72mPLAN\x1b[0m{footer}{indexed_footer}").as_bytes());
    let night_surface = Some(AnsiCode::RgbCode((39, 49, 61)));
    let dracula_surface = Some(AnsiCode::RgbCode((57, 58, 69)));
    yinz.wait_until("Codex surfaces use the launch theme", |grid| {
        ["COMPOSER", "SUBMITTED", "PLAN"]
            .iter()
            .all(|label| background(grid, label) == night_surface)
            && footer_matches(grid, night_footer)
    });
    // No provider output between rendering the editor and switching themes.
    for (config, expected, expected_footer) in [
        ("theme \"dracula\"", dracula_surface, dracula_footer),
        (
            "theme \"dayfox\"",
            Some(AnsiCode::RgbCode((231, 226, 226))),
            day_footer,
        ),
        ("theme \"nightfox\"", night_surface, night_footer),
    ] {
        let (code, error) = yinz.run_actions_reporting(vec![Action::Reconfigure {
            config: config.into(),
            write_to_disk: false,
        }]);
        assert_eq!(code, 0, "{error}");
        yinz.wait_until("existing Codex surfaces repaint", |grid| {
            ["COMPOSER", "SUBMITTED", "PLAN"]
                .iter()
                .all(|label| background(grid, label) == expected)
                && footer_matches(grid, expected_footer)
        });
    }
    // The overlay leaves the first pane column visible, so verify the actual editor fill.
    yinz.send_stdin(b"\x1b[112;7u");
    yinz.send_stdin(b"Change theme\r");
    yinz.wait_until("theme picker opens", |grid| grid.contains("Enter Save"));
    yinz.send_stdin(b"dracula");
    yinz.wait_until("preview repaints the retained editor", |grid| {
        grid.contains("> dracula")
            && background(grid, "COMPOSER") == dracula_surface
            && footer_matches(grid, dracula_footer)
    });
    yinz.send_stdin(b"\x1b");
    yinz.wait_until("cancel restores the retained editor", |grid| {
        grid.contains("Esc Close")
            && background(grid, "COMPOSER") == night_surface
            && footer_matches(grid, night_footer)
    });
    yinz.send_stdin(b"\x1b");
    yinz.wait_until("palette closes", |grid| {
        !grid.contains("Esc Close") && grid.contains("COMPOSER")
    });
    // Simulate Codex repainting a typed draft using its unchanged cached source color.
    codex.output(
        format!("\x1b[H\x1b[48;2;52;61;72mCOMPOSER typed\x1b[0m{footer}{indexed_footer}")
            .as_bytes(),
    );
    yinz.wait_until("typing retains semantic surface", |grid| {
        background(grid, "COMPOSER typed") == night_surface && footer_matches(grid, night_footer)
    });
    let (code, error) = yinz.run_actions_reporting(vec![Action::Reconfigure {
        config: "agent_theme_mapping \"off\"".into(),
        write_to_disk: false,
    }]);
    assert_eq!(code, 0, "{error}");
    yinz.wait_until("opt out restores original Codex fills", |grid| {
        background(grid, "COMPOSER typed") == Some(AnsiCode::RgbCode((52, 61, 72)))
            && background(grid, "SUBMITTED") == Some(AnsiCode::RgbCode((61, 70, 81)))
            && [
                ("MODEL", (246, 226, 183)),
                ("DIR", (171, 223, 167)),
                ("·", (133, 137, 143)),
            ]
            .into_iter()
            .all(|(label, rgb)| foreground(grid, label) == Some(AnsiCode::RgbCode(rgb)))
            && [("INDEX", 223), ("PATH", 151), (":", 245)]
                .into_iter()
                .all(|(label, index)| foreground(grid, label) == Some(AnsiCode::ColorIndex(index)))
    });
    let (code, error) = yinz.run_actions_reporting(vec![Action::Reconfigure {
        config: "agent_theme_mapping \"semantic\"".into(),
        write_to_disk: false,
    }]);
    assert_eq!(code, 0, "{error}");
    yinz.wait_until("opt in recolors the retained footer", |grid| {
        footer_matches(grid, night_footer)
    });
    yinz.quit();
}

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
    ordinary.output(b"\x1b[38;2;18;52;86mRAW-COLOR\x1b[0m\r\n");
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
            },
        );
    }
    yinz.quit();
}

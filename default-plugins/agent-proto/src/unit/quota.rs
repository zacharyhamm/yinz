use super::*;
use std::io::Cursor;

#[test]
fn quota_png_dimensions_color_antialiasing_and_digits() {
    let terminal = TerminalTheme {
        ansi: [(17, 43, 91); 16],
        foreground: (255, 255, 255),
        background: (0, 0, 0),
    };
    for value in (0..=9).chain([68, 100, 102, 999]) {
        let png = render(value, PaletteColor::EightBit(2), Some(terminal), 144, 36).unwrap();
        let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!(
            (info.width, info.height, info.color_type),
            (144, 36, png::ColorType::Rgba)
        );
        assert!(buffer
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[..3] == [17, 43, 91]));
        assert!(buffer
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[3] > 0 && p[3] < 255));
        assert!(buffer.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
        assert!(buffer.as_chunks::<4>().0.iter().any(|p| p[3] == 0));
    }
    for (color, rgb) in [
        (PaletteColor::Rgb((0, 0, 0)), [0, 0, 0]),
        (PaletteColor::Rgb((255, 255, 255)), [255, 255, 255]),
        (PaletteColor::EightBit(196), [255, 0, 0]),
    ] {
        for (width, height) in [(108, 24), (18, 2), (1, 1)] {
            let png = render(102, color, None, width, height).unwrap();
            let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
            let mut buffer = vec![0; reader.output_buffer_size()];
            let info = reader.next_frame(&mut buffer).unwrap();
            assert_eq!((info.width, info.height), (width, height));
            assert!(buffer.as_chunks::<4>().0.iter().all(|p| p[..3] == rgb));
        }
    }
}

#[test]
fn compact_digits_are_distinct_continuous_strokes() {
    let mut glyphs = Vec::new();
    for digit in 0..=9 {
        let png = render(digit, PaletteColor::Rgb((255, 255, 255)), None, 144, 36).unwrap();
        let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut buffer).unwrap();
        // The numeral sits in these columns, before the smaller percent sign.
        let mut ink = std::collections::BTreeSet::new();
        for y in 0..36 {
            for x in 106..129 {
                if buffer[(y * 144 + x) * 4 + 3] >= 128 {
                    ink.insert((x as i32, y as i32));
                }
            }
        }
        assert!(!ink.is_empty(), "missing digit {digit}");
        assert!(!glyphs.contains(&ink), "duplicate digit {digit}");
        glyphs.push(ink.clone());
        let mut pending = vec![ink.pop_first().unwrap()];
        while let Some((x, y)) = pending.pop() {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let neighbor = (x + dx, y + dy);
                    if ink.remove(&neighbor) {
                        pending.push(neighbor);
                    }
                }
            }
        }
        assert!(ink.is_empty(), "disconnected digit {digit}");
    }
}

#[test]
fn quota_png_rejects_unbounded_requests_before_allocation() {
    for (value, width, height) in [
        (68, 0, 60),
        (68, 1025, 60),
        (68, 100, 257),
        (68, 1024, 256),
        (u64::MAX, 160, 60),
        (68, u32::MAX, u32::MAX),
    ] {
        assert!(render(value, PaletteColor::EightBit(2), None, width, height).is_err());
    }
}

#[test]
fn quota_native_raster_matches_panel_png_for_remaining_boundaries() {
    for value in [0, 10, 25, 100] {
        let color = PaletteColor::Rgb((23, 47, 91));
        let png = render(value, color, None, 180, 40).unwrap();
        let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut buffer).unwrap();
        assert_eq!(buffer, render_rgba(value, color, None, 180, 40).unwrap());
        let usage = yinz_types::agent::control::ProviderUsage {
            provider: "codex".into(),
            status: yinz_types::agent::control::ProviderUsageStatus::Ok,
            observed_at_ms: Some(1),
            plan: None,
            message: None,
            windows: vec![yinz_types::agent::control::ProviderUsageWindow {
                label: "7d".into(),
                used_percent: (100 - value) as f64,
                resets_at_ms: Some(3_600_000),
            }],
        };
        let display = Display::new(&usage, 1);
        assert_eq!(display.value, Some(value));
        assert_eq!(display.compact, format!("Weekly · {value}%"));
        assert_eq!(display.reset, "59m left");
        let style = yinz_types::style::Style::default();
        assert_eq!(
            display.color(&style),
            match value {
                0 => style.colors.exit_code_error.base,
                10 | 25 => style.colors.text_unselected.emphasis_1,
                _ => style.colors.exit_code_success.base,
            }
        );
        let stale = Display::new(&usage, 120_001);
        assert_eq!(stale.weekly_label, "Weekly · stale");
        assert_eq!(stale.color(&style), style.colors.text_unselected.base);
    }
}

fn usage(windows: &[(&str, f64, Option<u64>)]) -> yinz_types::agent::control::ProviderUsage {
    use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};
    ProviderUsage {
        provider: "codex".into(),
        status: ProviderUsageStatus::Ok,
        observed_at_ms: Some(1_000),
        plan: None,
        message: Some("This message must not replace retained windows".into()),
        windows: windows
            .iter()
            .map(|&(label, used_percent, resets_at_ms)| ProviderUsageWindow {
                label: label.into(),
                used_percent,
                resets_at_ms,
            })
            .collect(),
    }
}

#[test]
fn quota_daily_and_custom_windows_keep_remaining_text_without_weekly_graphics() {
    for label in ["Daily limit", "Custom quota"] {
        let usage = usage(&[(label, 30.0, Some(3_601_000))]);
        let display = Display::new(&usage, 1_000);
        assert_eq!(display.compact, format!("{label} · 70%"));
        assert_eq!(display.other, [format!("{label} · 70%")]);
        assert_eq!(display.value, None);
        assert_eq!(display.remaining, None);
        assert_eq!(display.reset, "1h left");
        assert!(!display.stale);

        let stale = Display::new(&usage, 121_000);
        assert_eq!(stale.compact, format!("{label} · 70% · stale"));
        assert_eq!(stale.reset, "58m left");
        assert_eq!(stale.value, None);

        let mut failed = usage.clone();
        failed.status = yinz_types::agent::control::ProviderUsageStatus::Error;
        assert_eq!(Display::new(&failed, 1_000).compact, stale.compact);
    }
}

#[test]
fn quota_mixed_windows_keep_custom_text_and_prefer_weekly_reset() {
    let mut usage = usage(&[
        ("Daily limit", 30.0, Some(61_000)),
        ("5-hour limit", 80.0, None),
        ("7d", 75.0, Some(86_401_000)),
        ("Custom quota", 40.0, Some(3_601_000)),
    ]);
    let display = Display::new(&usage, 1_000);
    assert_eq!(display.value, Some(25));
    assert_eq!(display.short, "5hr · 20%");
    assert_eq!(
        display.compact,
        "Weekly · 25% · 5hr · 20% · Daily limit · 70% · Custom quota · 60%"
    );
    assert_eq!(display.reset, "1d left");
    usage.windows[2].resets_at_ms = None;
    assert_eq!(Display::new(&usage, 1_000).reset, "1m left");
    usage.windows[0].resets_at_ms = None;
    assert_eq!(Display::new(&usage, 1_000).reset, "1h left");
}

#[test]
fn quota_fallback_reset_text_covers_due_and_remaining_intervals() {
    for (reset, text) in [
        (0, "reset due"),
        (1_001, "<1m left"),
        (61_000, "1m left"),
        (3_601_000, "1h left"),
        (172_801_000, "2d left"),
    ] {
        assert_eq!(
            Display::new(&usage(&[("Daily limit", 30.0, Some(reset))]), 1_000).reset,
            text
        );
    }
    assert!(Display::new(&usage(&[("Daily limit", 30.0, None)]), 1_000)
        .reset
        .is_empty());
}

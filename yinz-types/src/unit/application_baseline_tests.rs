use super::*;

#[test]
fn fallbacks_use_xterm_defaults_for_each_mode() {
    let dark = ApplicationBaseline::fallback(HostTerminalThemeMode::Dark);
    assert_eq!(dark.mode, HostTerminalThemeMode::Dark);
    assert_eq!(dark.theme.foreground, (0xe5, 0xe5, 0xe5));
    assert_eq!(dark.theme.background, (0, 0, 0));
    let light = ApplicationBaseline::fallback(HostTerminalThemeMode::Light);
    assert_eq!(light.mode, HostTerminalThemeMode::Light);
    assert_eq!(light.theme.foreground, (0, 0, 0));
    assert_eq!(light.theme.background, (0xff, 0xff, 0xff));
    assert_eq!(dark.theme.ansi, light.theme.ansi);
    assert_eq!(dark.register(1), (0xcd, 0, 0));
    assert_eq!(dark.register(12), (0x5c, 0x5c, 0xff));
    assert_eq!(dark.register(15), (0xff, 0xff, 0xff));
    assert_eq!(ApplicationBaseline::default(), dark);
}

#[test]
fn registers_above_fifteen_follow_the_xterm_cube_and_grayscale() {
    let mut baseline = ApplicationBaseline::default();
    baseline.theme.ansi = [(1, 2, 3); 16];
    assert_eq!(baseline.register(0), (1, 2, 3));
    assert_eq!(baseline.register(16), (0, 0, 0));
    assert_eq!(baseline.register(17), (0, 0, 95));
    assert_eq!(baseline.register(21), (0, 0, 255));
    assert_eq!(baseline.register(22), (0, 95, 0));
    assert_eq!(baseline.register(52), (95, 0, 0));
    assert_eq!(baseline.register(200), (255, 0, 215));
    assert_eq!(baseline.register(231), (255, 255, 255));
    assert_eq!(baseline.register(232), (8, 8, 8));
    assert_eq!(baseline.register(244), (128, 128, 128));
    assert_eq!(baseline.register(255), (238, 238, 238));
}

#[test]
fn application_baseline_is_not_serialized_with_style() {
    let style = Style {
        application: ApplicationBaseline::fallback(HostTerminalThemeMode::Light),
        ..Default::default()
    };
    let json = serde_json::to_string(&style).unwrap();
    assert_eq!(json, serde_json::to_string(&Style::default()).unwrap());
    let back: Style = serde_json::from_str(&json).unwrap();
    assert_eq!(back.application, ApplicationBaseline::default());
}

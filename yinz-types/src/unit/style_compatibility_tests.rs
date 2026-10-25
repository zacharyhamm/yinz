use super::*;

#[test]
fn style_values_preserve_json_defaults_and_theme_aliases() {
    let json = r#"{"base":{"EightBit":0},"background":{"EightBit":0},"emphasis_0":{"EightBit":0},"emphasis_1":{"EightBit":0},"emphasis_2":{"EightBit":0},"emphasis_3":{"EightBit":0}}"#;
    assert_eq!(
        serde_json::to_string(&StyleDeclaration::default()).unwrap(),
        json
    );
    assert_eq!(
        serde_json::from_str::<StyleDeclaration>(json).unwrap(),
        StyleDeclaration::default()
    );
    for (hue, json, alias) in [
        (ThemeHue::Dark, "\"Dark\"", "\"dark\""),
        (ThemeHue::Light, "\"Light\"", "\"light\""),
    ] {
        assert_eq!(serde_json::to_string(&hue).unwrap(), json);
        assert_eq!(serde_json::from_str::<ThemeHue>(alias).unwrap(), hue);
    }
    let style = Style {
        terminal: None,
        rounded_corners: true,
        hide_session_name: false,
        ..Default::default()
    };
    let value = serde_json::to_value(style).unwrap();
    assert_eq!(value["rounded_corners"], true);
    assert_eq!(value["hide_session_name"], false);
    assert_eq!(serde_json::from_value::<Style>(value).unwrap(), style);
    assert_eq!(
        detect_theme_hue(PaletteColor::Rgb((0, 0, 0))),
        ThemeHue::Dark
    );
    assert_eq!(
        detect_theme_hue(PaletteColor::Rgb((255, 255, 255))),
        ThemeHue::Light
    );
}

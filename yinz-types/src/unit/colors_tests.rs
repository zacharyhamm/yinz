use super::*;

#[test]
fn color_json_keeps_variant_names_and_channel_widths() {
    for (color, json) in [
        (PaletteColor::EightBit(0), r#"{"EightBit":0}"#),
        (PaletteColor::EightBit(255), r#"{"EightBit":255}"#),
        (PaletteColor::Rgb((0, 127, 255)), r#"{"Rgb":[0,127,255]}"#),
    ] {
        assert_eq!(serde_json::to_string(&color).unwrap(), json);
        assert_eq!(serde_json::from_str::<PaletteColor>(json).unwrap(), color);
    }
    assert!(serde_json::from_str::<PaletteColor>(r#"{"EightBit":256}"#).is_err());
    assert!(serde_json::from_str::<PaletteColor>(r#"{"Rgb":[-1,0,0]}"#).is_err());
}

use super::*;
use prost::Message;

#[test]
fn pane_surround_protobuf_roundtrip() {
    for pane_surround in [
        None,
        Some(PaletteColor::EightBit(0)),
        Some(PaletteColor::Rgb((28, 30, 42))),
    ] {
        let style = Styling {
            pane_surround,
            ..Default::default()
        };
        let encoded = ProtobufStyling::try_from(style).unwrap().encode_to_vec();
        let decoded = ProtobufStyling::decode(encoded.as_slice()).unwrap();
        assert_eq!(Styling::try_from(decoded).unwrap(), style);
    }
    let mut invalid = ProtobufStyling::try_from(Styling::default()).unwrap();
    invalid.pane_surround = Some(ProtobufColor {
        color_type: ProtobufColorType::EightBit as i32,
        payload: Some(ProtobufColorPayload::EightBitColorPayload(256)),
    });
    assert!(Styling::try_from(invalid).is_err());
}

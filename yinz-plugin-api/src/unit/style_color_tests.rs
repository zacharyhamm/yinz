use super::*;
use prost::Message;

#[test]
fn colors_keep_their_protobuf_bytes_and_reject_narrowing_overflow() {
    for (color, bytes) in [
        (PaletteColor::EightBit(0), vec![8, 1, 24, 0]),
        (PaletteColor::EightBit(255), vec![8, 1, 24, 255, 1]),
        (
            PaletteColor::Rgb((0, 127, 255)),
            vec![18, 5, 16, 127, 24, 255, 1],
        ),
    ] {
        let protobuf = ProtobufColor::try_from(color).unwrap();
        assert_eq!(protobuf.encode_to_vec(), bytes);
        let decoded = ProtobufColor::decode(bytes.as_slice()).unwrap();
        assert_eq!(PaletteColor::try_from(decoded).unwrap(), color);
    }
    let too_large = ProtobufColor {
        color_type: ProtobufColorType::EightBit as i32,
        payload: Some(ProtobufColorPayload::EightBitColorPayload(256)),
    };
    assert_eq!(
        PaletteColor::try_from(too_large),
        Err("8bit color index out of range")
    );
    for channel in 0..3 {
        let mut rgb = ProtobufRgbColorPayload::default();
        match channel {
            0 => rgb.red = 256,
            1 => rgb.green = 256,
            _ => rgb.blue = 256,
        }
        let too_large = ProtobufColor {
            color_type: ProtobufColorType::Rgb as i32,
            payload: Some(ProtobufColorPayload::RgbColorPayload(rgb)),
        };
        assert_eq!(
            PaletteColor::try_from(too_large),
            Err("RGB color channel out of range")
        );
    }
}

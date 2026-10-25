use super::*;

#[test]
fn pane_frame_style_roundtrips_through_protobuf() {
    for style in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        let protobuf: ProtobufPaneFrameStyle = style.try_into().unwrap();
        let roundtripped: PaneFrameStyle = protobuf.try_into().unwrap();
        assert_eq!(style, roundtripped);
    }
}

#[test]
fn unknown_pane_frame_style_value_is_rejected() {
    assert!(ProtobufPaneFrameStyle::try_from(99).is_err());
}

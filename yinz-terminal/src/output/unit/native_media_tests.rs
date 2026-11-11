use super::*;

#[test]
fn native_disabled_styled_underlines_keep_default_color() {
    let mut palette = [None; 256];
    palette[1] = Some(AnsiCode::RgbCode((10, 20, 30)));
    let styles = CharacterStyles {
        underline_color: Some(AnsiCode::NamedColor(NamedColor::Red)),
        ..DEFAULT_STYLES
    };
    let cell = wire_cell(&TerminalCharacter::new('x'), styles, Some(&palette), false);
    assert_eq!(WireColor::unpack(cell.underline_color), WireColor::Default);
}

#[test]
fn shared_uploads_survive_until_the_frame_is_acknowledged() {
    let first = tempfile::NamedTempFile::new().unwrap();
    let second = tempfile::NamedTempFile::new().unwrap();
    let first_path = first.path().to_owned();
    let second_path = second.path().to_owned();
    let mut state = NativeRenderState {
        seq: 2,
        media: vec![(1, first), (2, second)],
        ..Default::default()
    };
    state.acknowledge(1);
    assert!(!first_path.exists());
    assert!(second_path.exists());
    state.acknowledge(3);
    assert!(second_path.exists());
    state.acknowledge(2);
    assert!(!second_path.exists());
}

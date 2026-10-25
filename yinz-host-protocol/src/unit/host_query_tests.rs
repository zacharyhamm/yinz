use super::*;

#[test]
fn text_area_pixel_size_serializes_as_csi_14t() {
    assert_eq!(HostQuery::TextAreaPixelSize.to_query_bytes(), b"\x1b[14t");
}

#[test]
fn character_cell_pixel_size_serializes_as_csi_16t() {
    assert_eq!(
        HostQuery::CharacterCellPixelSize.to_query_bytes(),
        b"\x1b[16t"
    );
}

#[test]
fn default_foreground_mirrors_terminator() {
    assert_eq!(
        HostQuery::DefaultForeground {
            terminator: OscTerminator::St,
        }
        .to_query_bytes(),
        b"\x1b]10;?\x1b\\",
    );
    assert_eq!(
        HostQuery::DefaultForeground {
            terminator: OscTerminator::Bel,
        }
        .to_query_bytes(),
        b"\x1b]10;?\x07",
    );
}

#[test]
fn default_background_serializes_correctly() {
    assert_eq!(
        HostQuery::DefaultBackground {
            terminator: OscTerminator::St,
        }
        .to_query_bytes(),
        b"\x1b]11;?\x1b\\",
    );
}

#[test]
fn palette_register_carries_index_and_terminator() {
    assert_eq!(
        HostQuery::PaletteRegister {
            index: 5,
            terminator: OscTerminator::Bel,
        }
        .to_query_bytes(),
        b"\x1b]4;5;?\x07",
    );
    assert_eq!(
        HostQuery::PaletteRegister {
            index: 255,
            terminator: OscTerminator::St,
        }
        .to_query_bytes(),
        b"\x1b]4;255;?\x1b\\",
    );
}

#[test]
fn clipboard_content_carries_selection_and_terminator() {
    assert_eq!(
        HostQuery::ClipboardContent {
            selection: 'c',
            terminator: OscTerminator::Bel,
        }
        .to_query_bytes(),
        b"\x1b]52;c;?\x07",
    );
    assert_eq!(
        HostQuery::ClipboardContent {
            selection: 'p',
            terminator: OscTerminator::St,
        }
        .to_query_bytes(),
        b"\x1b]52;p;?\x1b\\",
    );
}

#[test]
fn clipboard_content_empty_reply_mirrors_the_query_shape() {
    assert_eq!(
        HostQuery::ClipboardContent {
            selection: 'c',
            terminator: OscTerminator::Bel,
        }
        .empty_reply_bytes(),
        b"\x1b]52;c;\x07",
    );
    assert_eq!(
        HostQuery::ClipboardContent {
            selection: 'p',
            terminator: OscTerminator::St,
        }
        .empty_reply_bytes(),
        b"\x1b]52;p;\x1b\\",
    );
}

#[test]
fn non_clipboard_queries_have_no_empty_reply() {
    assert!(HostQuery::TextAreaPixelSize.empty_reply_bytes().is_empty());
    assert!(HostQuery::DefaultBackground {
        terminator: OscTerminator::St,
    }
    .empty_reply_bytes()
    .is_empty());
}

#[test]
fn from_bell_terminated_maps_bool_to_variant() {
    assert_eq!(
        OscTerminator::from_bell_terminated(true),
        OscTerminator::Bel
    );
    assert_eq!(
        OscTerminator::from_bell_terminated(false),
        OscTerminator::St
    );
}

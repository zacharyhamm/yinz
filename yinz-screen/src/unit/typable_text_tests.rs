use super::is_typable_text;
use crate::panes::terminal_pane::{BRACKETED_PASTE_BEGIN, BRACKETED_PASTE_END};

#[test]
fn text_and_the_editing_keys_a_name_understands_are_typable() {
    // `\0` clears a name and DEL/BS delete from it; `update_active_tab_name` reads all three.
    for bytes in [
        &b"a"[..],
        b"!",
        b" ",
        "\u{e9}".as_bytes(),
        b"\0",
        b"\x7f",
        b"\x08",
    ] {
        assert!(is_typable_text(bytes), "{bytes:?} should be typable");
    }
}

#[test]
fn an_escape_sequence_is_not_typable() {
    for bytes in [
        // A key with no text of its own, under either kitty flag set.
        &b"\x1b[97;3u"[..],
        // A host that ignored REPORT_ALTERNATE_KEYS, leaving a shifted key unresolved.
        b"\x1b[49;2u",
        b"\x1b[D",
        BRACKETED_PASTE_BEGIN,
        BRACKETED_PASTE_END,
    ] {
        assert!(!is_typable_text(bytes), "{bytes:?} should not be typable");
    }
}

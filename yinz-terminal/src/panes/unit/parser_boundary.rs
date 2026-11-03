use super::*;

#[test]
fn every_split_of_common_sequences_refuses_an_incomplete_snapshot() {
    for sequence in [
        b"\x1b[31m".as_slice(),
        b"\x1b]2;title\x07",
        b"\x1bPdata\x1b\\",
        b"\x1bPz\x9c",
        b"\x1b(0",
        "🙂".as_bytes(),
    ] {
        for split in 1..sequence.len() {
            let mut boundary = ParserBoundary::default();
            boundary.advance(&sequence[..split]);
            assert!(!boundary.is_ground(), "{sequence:?} at {split}");
            boundary.advance(&sequence[split..]);
            assert!(boundary.is_ground(), "{sequence:?} at {split}");
        }
    }
}

#[test]
fn cancellation_and_invalid_utf8_do_not_hide_following_escape_sequences() {
    let mut boundary = ParserBoundary::default();
    boundary.advance(b"\x1b[12\x18");
    assert!(boundary.is_ground());
    boundary.advance(b"\xf0\x1b[32");
    assert!(!boundary.is_ground());
    boundary.advance(b"mtext");
    assert!(boundary.is_ground());
    boundary.advance(b"\x1b]payload\x1b");
    assert!(!boundary.is_ground());
    boundary.advance(b"\\");
    assert!(boundary.is_ground());
}

#[test]
fn dcs_eight_bit_terminator_matches_vte_at_every_split() {
    #[derive(Default)]
    struct Printed(String);
    impl vte::Perform for Printed {
        fn print(&mut self, character: char) {
            self.0.push(character);
        }
    }
    let bytes = b"\x1bPz\x9cOK";
    for split in 0..=bytes.len() {
        let mut parser = vte::Parser::new();
        let mut printed = Printed::default();
        let mut boundary = ParserBoundary::default();
        for chunk in [&bytes[..split], &bytes[split..]] {
            parser.advance(&mut printed, chunk);
            boundary.advance(chunk);
        }
        assert_eq!(printed.0, "OK");
        assert!(boundary.is_ground());
    }
    // vte only accepts ST in DCS passthrough, not in an unfinished or invalid header.
    for bytes in [
        b"\x1bP\x9c".as_slice(),
        b"\x1bP1\x9c",
        b"\x1bP 1z\x9c",
        b"\x1b]title\x9c",
        b"\x1b_hidden\x9c",
    ] {
        let mut boundary = ParserBoundary::default();
        boundary.advance(bytes);
        assert!(!boundary.is_ground(), "{bytes:?}");
    }
}

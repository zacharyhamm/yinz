//! Response format conformance.
//!
//! Spec: a reply is an APC of the form `<ESC>_G<control data>;<response><ESC>\`, where the
//! response is the literal `OK` or an errno-style code followed by `:<message>` — "the message
//! data will be a ASCII encoded string containing only printable characters and spaces". Also the
//! `q` suppression levels, and the rule that a transmission carrying no identifier gets no
//! response at all. Derived from kitty's `test_suppressing_gr_command_responses`.
//!
//! These tests double as the check that the shared reply parser in `test_support` agrees with
//! `format_kitty_reply` / `format_kitty_error`, so every other level can assert on parsed replies
//! instead of raw bytes.

use super::super::parser::{KittyCommandParser, KittyError, KittyErrorCode};
use super::super::replies::{format_kitty_error, format_kitty_reply};
use super::super::test_support::{apc_b64, unwrap_apc, KittyReply, KittyReplyParseError};
use crate::panes::kitty_graphics::grid_state::KittyReplyData;

fn reply_data(
    image_id: Option<u32>,
    image_number: Option<u32>,
    placement_id: Option<u32>,
) -> KittyReplyData {
    KittyReplyData {
        image_id,
        image_number,
        placement_id,
        quiet: 0,
    }
}

fn error(code: KittyErrorCode, message: &str) -> KittyError {
    KittyError {
        code,
        message: message.to_owned(),
        image_id: Some(3),
        image_number: None,
        placement_id: None,
        quiet: 0,
    }
}

fn parsed(bytes: Option<Vec<u8>>) -> Option<KittyReply> {
    bytes.map(|bytes| KittyReply::parse(&bytes).expect("well-formed reply"))
}

// ---- success responses ----

#[test]
fn transmit_with_an_image_id_answers_ok_echoing_the_id() {
    let bytes = format_kitty_reply(&reply_data(Some(31), None, None), false).unwrap();
    assert_eq!(bytes, b"\x1b_Gi=31;OK\x1b\\");
    let reply = KittyReply::parse(&bytes).unwrap();
    assert!(reply.is_ok());
    assert_eq!(reply.image_id, Some(31));
    assert_eq!(reply.image_number, None);
    assert_eq!(reply.placement_id, None);
}

#[test]
fn a_placement_id_is_echoed_alongside_the_image_id() {
    let reply = parsed(format_kitty_reply(
        &reply_data(Some(7), None, Some(17)),
        false,
    ))
    .unwrap();
    assert!(reply.is_ok());
    assert_eq!((reply.image_id, reply.placement_id), (Some(7), Some(17)));
}

#[test]
fn an_image_number_response_reports_the_assigned_id_and_the_number() {
    let reply = parsed(format_kitty_reply(
        &reply_data(Some(99), Some(13), None),
        false,
    ))
    .unwrap();
    assert!(reply.is_ok());
    assert_eq!((reply.image_id, reply.image_number), (Some(99), Some(13)));
}

#[test]
fn keys_are_emitted_in_the_canonical_i_then_capital_i_then_p_order() {
    let bytes = format_kitty_reply(&reply_data(Some(1), Some(2), Some(3)), false).unwrap();
    assert_eq!(bytes, b"\x1b_Gi=1,I=2,p=3;OK\x1b\\");
}

#[test]
fn an_anonymous_transmission_gets_no_response() {
    // Spec: a response is only sent when the command carried an identifier, so a client that
    // never names its images never has to drain replies.
    assert_eq!(
        format_kitty_reply(&reply_data(None, None, None), false),
        None
    );
    assert_eq!(
        format_kitty_reply(&reply_data(Some(0), None, None), false),
        None
    );
}

#[test]
fn a_query_always_answers_even_without_an_identifier() {
    // The query action is how clients detect support at all, so it cannot be silent.
    let reply = parsed(format_kitty_reply(&reply_data(None, None, None), true)).unwrap();
    assert!(reply.is_ok());
}

// ---- error responses ----

#[test]
fn errors_are_reported_as_code_colon_message() {
    let bytes = format_kitty_error(&error(KittyErrorCode::Enoent, "unknown image id")).unwrap();
    assert_eq!(bytes, b"\x1b_Gi=3;ENOENT:unknown image id\x1b\\");
    let reply = KittyReply::parse(&bytes).unwrap();
    assert!(!reply.is_ok());
    assert_eq!(reply.code, "ENOENT");
    assert_eq!(reply.message, "unknown image id");
}

#[test]
fn every_error_code_renders_as_its_errno_name() {
    for (code, name) in [
        (KittyErrorCode::Einval, "EINVAL"),
        (KittyErrorCode::Enoent, "ENOENT"),
        (KittyErrorCode::Ebadf, "EBADF"),
        (KittyErrorCode::Ebadpng, "EBADPNG"),
        (KittyErrorCode::Enodata, "ENODATA"),
        (KittyErrorCode::Enospc, "ENOSPC"),
        (KittyErrorCode::Enotsupported, "ENOTSUPPORTED"),
    ] {
        let reply = parsed(format_kitty_error(&error(code, "boom"))).unwrap();
        assert_eq!(reply.code, name);
    }
}

/// Every distinct way `parse_control_data` and `run_payload_pipeline` can refuse a command, so the
/// sweep below covers the message literals rather than one written for the occasion.
fn every_way_the_parser_can_refuse() -> Vec<(&'static str, &'static str, Vec<u8>)> {
    let short_png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a];
    vec![
        ("malformed control data", "a=p,i", vec![]),
        ("invalid action", "a=Z", vec![]),
        ("multi-character action", "a=tt", vec![]),
        ("invalid format", "f=8,s=1,v=1", vec![0; 3]),
        ("invalid transmission medium", "t=q,s=1,v=1", vec![0; 4]),
        ("invalid compression", "f=24,s=1,v=1,o=q", vec![0; 3]),
        ("invalid chunk marker", "f=24,s=1,v=1,m=7", vec![0; 3]),
        ("invalid delete specifier", "a=d,d=k", vec![]),
        ("non-numeric width", "s=x,v=1", vec![]),
        ("non-numeric z-index", "a=p,i=1,z=nope", vec![]),
        ("invalid quiet level", "a=p,i=1,q=9", vec![]),
        ("invalid cursor policy", "a=p,i=1,C=9", vec![]),
        ("invalid virtual placement flag", "a=p,i=1,U=x", vec![]),
        (
            "both an id and a number",
            "f=24,s=1,v=1,i=1,I=2",
            vec![0; 3],
        ),
        ("missing dimensions", "a=T,f=24,i=1", vec![0; 3]),
        (
            "dimensions too large",
            "a=T,f=32,s=99999,v=99999,i=1",
            vec![],
        ),
        ("insufficient data", "a=T,f=24,s=10,v=10,i=1", vec![0; 3]),
        (
            "payload that will not inflate",
            "a=T,f=24,s=2,v=2,i=1,o=z",
            b"not deflate".to_vec(),
        ),
        ("invalid png data", "a=T,f=100,i=1", short_png),
        (
            "unreadable file",
            "a=T,f=24,s=2,v=2,i=1,t=f",
            b"/nonexistent/zellij/kitty".to_vec(),
        ),
        (
            "animation is unsupported",
            "a=f,f=24,s=1,v=1,i=1",
            vec![0; 3],
        ),
        (
            "could not read shared memory object",
            "a=T,f=24,s=1,v=1,i=1,t=s",
            b"/zellij-kitty-no-such-object".to_vec(),
        ),
        (
            "unicode placeholders are unsupported",
            "a=p,i=1,U=1",
            vec![],
        ),
    ]
}

#[test]
fn every_message_the_parser_emits_stays_within_printable_ascii() {
    // Spec, "Querying support and available transmission mediums": "The message data will be a
    // ASCII encoded string containing only printable characters and spaces." A message carrying a
    // control byte — or the `;` that separates a reply's control data from its response — would
    // corrupt the framing of the reply and of everything the application read after it.
    //
    // Swept over messages the parser really produces, so a message literal added to `parser.rs`
    // later is covered without anyone remembering to come back here. `KittyReply::parse` is what
    // enforces the rule; this test is what makes sure every message reaches it.
    for (description, control, payload) in every_way_the_parser_can_refuse() {
        let mut parser = KittyCommandParser::new();
        let error = parser
            .parse(unwrap_apc(&apc_b64(control, &payload)))
            .unwrap_or_else(|| panic!("{}: {:?} left an upload in flight", description, control))
            .expect_err(description);
        let bytes = format_kitty_error(&error)
            .unwrap_or_else(|| panic!("{}: a q=0 error must be reported", description));

        let reply = KittyReply::parse(&bytes).unwrap_or_else(|e| {
            panic!(
                "{}: {:?} produced a malformed reply {:?}: {:?}",
                description,
                control,
                String::from_utf8_lossy(&bytes),
                e
            )
        });
        assert!(
            !reply.message.is_empty(),
            "{}: {:?} refused the command without saying why",
            description,
            control
        );
    }
}

#[test]
fn a_message_that_would_corrupt_the_framing_is_not_a_well_formed_reply() {
    // The teeth behind the sweep above: these are what `KittyReply::parse` is checking for.
    assert!(matches!(
        KittyReply::parse(b"\x1b_Gi=1;EINVAL:two\x1b[0mparts\x1b\\"),
        Err(KittyReplyParseError::NonPrintableMessage(_))
    ));
    assert!(matches!(
        KittyReply::parse(b"\x1b_Gi=1;EINVAL:semi;colon\x1b\\"),
        Err(KittyReplyParseError::NonPrintableMessage(_))
    ));
}

// ---- quiet levels ----

#[test]
fn quiet_one_suppresses_success_but_not_errors() {
    let mut data = reply_data(Some(1), None, None);
    data.quiet = 1;
    assert_eq!(format_kitty_reply(&data, false), None);
    assert_eq!(format_kitty_reply(&data, true), None);

    let mut err = error(KittyErrorCode::Einval, "bad");
    err.quiet = 1;
    assert!(format_kitty_error(&err).is_some());
}

#[test]
fn quiet_two_suppresses_errors_as_well() {
    let mut data = reply_data(Some(1), None, None);
    data.quiet = 2;
    assert_eq!(format_kitty_reply(&data, false), None);

    let mut err = error(KittyErrorCode::Einval, "bad");
    err.quiet = 2;
    assert_eq!(format_kitty_error(&err), None);
}

#[test]
fn quiet_zero_is_the_default_and_reports_everything() {
    assert!(format_kitty_reply(&reply_data(Some(1), None, None), false).is_some());
    assert!(format_kitty_error(&error(KittyErrorCode::Einval, "bad")).is_some());
}

// ---- the parser used by the rest of the suite ----

#[test]
fn the_reply_parser_rejects_anything_that_is_not_a_terminated_apc() {
    assert_eq!(
        KittyReply::parse(b"\x1bPi=1;OK\x1b\\"),
        Err(KittyReplyParseError::NotAnApc)
    );
    assert_eq!(
        KittyReply::parse(b"\x1b_Gi=1;OK"),
        Err(KittyReplyParseError::Unterminated)
    );
    assert_eq!(
        KittyReply::parse(b"\x1b_Gi=1\x1b\\"),
        Err(KittyReplyParseError::MissingSemicolon)
    );
}

#[test]
fn the_reply_parser_rejects_keys_it_cannot_interpret() {
    // This is what makes "the implementation only ever emits keys the protocol defines for a
    // response" an assertable property at every other level of the suite.
    assert_eq!(
        KittyReply::parse(b"\x1b_Gi=1,zz=4;OK\x1b\\"),
        Err(KittyReplyParseError::UnknownKey("zz".to_owned()))
    );
    assert_eq!(
        KittyReply::parse(b"\x1b_Gi=x;OK\x1b\\"),
        Err(KittyReplyParseError::NonNumericValue("i=x".to_owned()))
    );
}

#[test]
fn several_replies_in_one_stream_are_split_apart() {
    let mut stream = Vec::new();
    stream.extend_from_slice(b"\x1b_Gi=1;OK\x1b\\");
    stream.extend_from_slice(b"some unrelated output");
    stream.extend_from_slice(b"\x1b_Gi=2,p=3;ENOENT:gone\x1b\\");
    let replies = super::super::test_support::parse_replies(&stream);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0].image_id, Some(1));
    assert_eq!(replies[1].code, "ENOENT");
    assert_eq!(replies[1].placement_id, Some(3));
}

//! Byte-level helpers for driving the kitty graphics protocol in whole-app tests.
//!
//! The command builders and the reply grammar are the conformance suite's own, from
//! `yinz_terminal_graphics::kitty_graphics::test_support`. What this module adds is for reading
//! a live pty stream: [`parse_replies`] handles a malformed reply differently, as documented
//! there, and the byte-search helpers back the assertions that the terminal said nothing.

pub use yinz_terminal_graphics::kitty_graphics::test_support::{
    apc, apc_b64, apc_raw, rgb_block, KittyReply,
};

/// Every well-formed graphics reply in a byte stream, in order.
///
/// Unlike `test_support::parse_replies`, this one *skips* anything it cannot parse rather than
/// panicking: it is handed a live pty stream whose last sequence may still be arriving, and it
/// runs inside polling predicates where a panic would be a hard test failure rather than a retry.
/// The cost is that "no reply" and "a malformed reply" look alike here, so a test asserting
/// silence must also check the raw bytes — see [`contains_graphics_apc`].
pub fn parse_replies(bytes: &[u8]) -> Vec<KittyReply> {
    let mut replies = Vec::new();
    let mut rest = bytes;
    while let Some(start) = find_bytes(rest, b"\x1b_G") {
        let after_start = &rest[start..];
        let end = match find_bytes(after_start, b"\x1b\\") {
            Some(end) => end + 2,
            None => break,
        };
        if let Ok(reply) = KittyReply::parse(&after_start[..end]) {
            replies.push(reply);
        }
        rest = &after_start[end..];
    }
    replies
}

/// Whether the stream contains a graphics APC at all, parseable or not. The complement to
/// [`parse_replies`] for tests that assert the terminal said nothing.
pub fn contains_graphics_apc(bytes: &[u8]) -> bool {
    contains_bytes(bytes, b"\x1b_G")
}

pub fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    find_bytes(haystack, needle).is_some()
}

pub fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Standard padded base64, for tests that build kitty payloads by hand.
pub fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Wait for the applied capability; keyboard input can overtake the server's host reply.
pub fn wait_for_kitty_host(terminal: &crate::FakePtyHandle) {
    let replies = terminal.tap_stdin();
    let deadline = std::time::Instant::now() + crate::default_timeout();
    let query = b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\";
    terminal.output(query);
    let mut bytes = Vec::new();
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "kitty capability remained unsupported"
        );
        let chunk = replies.recv_deadline(deadline).unwrap_or_else(|error| {
            panic!(
                "kitty probe acknowledgement: {error}; replies={:?}\n{}",
                String::from_utf8_lossy(&bytes),
                crate::test_env::log_tail(40),
            )
        });
        bytes.extend(chunk);
        if let Some(reply) = parse_replies(&bytes)
            .into_iter()
            .find(|r| r.image_id == Some(31))
        {
            if reply.is_ok() {
                return;
            }
            assert_eq!(
                reply.code, "ENOTSUPPORTED",
                "unexpected probe reply: {reply:?}"
            );
            log::info!("kitty handshake: capability not applied yet; {reply:?}");
            bytes.clear();
            terminal.output(query);
        }
    }
}

//! Byte-level helpers for driving the kitty graphics protocol in whole-app tests.
//!
//! This duplicates a small part of `zellij-server`'s
//! `panes::kitty_graphics::test_support`. It has to: that module is `#[cfg(test)]`, so it is not
//! in the `zellij-server` rlib, and this crate already depends on `zellij-server`, which makes
//! the reverse edge a cycle. Keep the reply *grammar* here in step with the one over there; the
//! one deliberate difference is how a malformed reply is handled, documented on [`parse_replies`].

/// `ESC _G <control> ESC \` — a command with no payload.
pub fn apc(control: &str) -> Vec<u8> {
    apc_raw(control, &[])
}

/// `ESC _G <control> ; base64(payload) ESC \`.
pub fn apc_b64(control: &str, payload: &[u8]) -> Vec<u8> {
    apc_raw(control, base64_encode(payload).as_bytes())
}

/// Like [`apc_b64`] but the payload is emitted verbatim.
pub fn apc_raw(control: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b_G");
    out.extend_from_slice(control.as_bytes());
    if !payload.is_empty() {
        out.push(b';');
        out.extend_from_slice(payload);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// A terminal-to-application graphics response.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KittyReply {
    pub code: String,
    pub message: String,
    pub image_id: Option<u32>,
    pub image_number: Option<u32>,
    pub placement_id: Option<u32>,
    pub frame_number: Option<u32>,
}

impl KittyReply {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let body = bytes.strip_prefix(b"\x1b_G".as_slice())?;
        let body = body.strip_suffix(b"\x1b\\".as_slice())?;
        let semicolon = body.iter().position(|byte| *byte == b';')?;
        let (keys, status) = (&body[..semicolon], &body[semicolon + 1..]);

        let status = String::from_utf8_lossy(status).into_owned();
        let (code, message) = match status.split_once(':') {
            Some((code, message)) => (code.to_owned(), message.to_owned()),
            None => (status, String::new()),
        };

        // Spec: the message is "a ASCII encoded string containing only printable characters and
        // spaces". Kept in step with `zellij-server`'s `test_support`, which rejects the same
        // shapes — one carrying a control byte or a `;` would corrupt the framing of the replies
        // after it.
        if !message
            .bytes()
            .all(|byte| (0x20..0x7f).contains(&byte) && byte != b';')
        {
            return None;
        }

        let mut reply = KittyReply {
            code,
            message,
            ..Default::default()
        };
        for segment in keys.split(|byte| *byte == b',') {
            if segment.is_empty() {
                continue;
            }
            let segment = String::from_utf8_lossy(segment).into_owned();
            let (key, value) = segment.split_once('=')?;
            let value = value.parse::<u32>().ok()?;
            match key {
                "i" => reply.image_id = Some(value),
                "I" => reply.image_number = Some(value),
                "p" => reply.placement_id = Some(value),
                "r" => reply.frame_number = Some(value),
                _ => return None,
            }
        }
        Some(reply)
    }

    pub fn is_ok(&self) -> bool {
        self.code == "OK"
    }
}

/// Every well-formed graphics reply in a byte stream, in order.
///
/// Unlike `zellij-server`'s `test_support::parse_replies`, this one *skips* anything it cannot
/// parse rather than panicking: it is handed a live pty stream whose last sequence may still be
/// arriving, and it runs inside polling predicates where a panic would be a hard test failure
/// rather than a retry. The cost is that "no reply" and "a malformed reply" look alike here, so a
/// test asserting silence must also check the raw bytes — see [`contains_graphics_apc`].
pub fn parse_replies(bytes: &[u8]) -> Vec<KittyReply> {
    let mut replies = Vec::new();
    let mut rest = bytes;
    while let Some(start) = find_bytes(rest, b"\x1b_G") {
        let after_start = &rest[start..];
        let end = match find_bytes(after_start, b"\x1b\\") {
            Some(end) => end + 2,
            None => break,
        };
        if let Some(reply) = KittyReply::parse(&after_start[..end]) {
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

/// Standard base64, written out so the test crate needs no encoder dependency.
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

/// `width * height * 3` deterministic RGB bytes.
pub fn rgb_block(width: usize, height: usize) -> Vec<u8> {
    (0..width * height * 3).map(|i| (i % 256) as u8).collect()
}

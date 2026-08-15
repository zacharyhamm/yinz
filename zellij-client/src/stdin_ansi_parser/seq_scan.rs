//! Byte-level scanning of escape sequences.
//!
//! These routines answer one question about the head of a buffer: is there a
//! complete sequence here, the start of one that needs more bytes, or
//! something malformed. Telling "needs more" apart from "malformed" is what
//! lets the parser buffer a sequence split across reads instead of leaking
//! its bytes into keyboard residue.

/// Cap on the size of an in-flight partial OSC/CSI buffer. Sized to
/// pass legitimate OSC 52 clipboard payloads, which carry the entire
/// clipboard base64-encoded and have no protocol-level limit (images,
/// multi-MB text dumps, etc.). Beyond this we assume a runaway or
/// malformed sequence and flush the buffered bytes back to residue —
/// same observable behaviour as today for unterminated OSC, just
/// bounded.
pub(super) const PARTIAL_BUFFER_CAP_BYTES: usize = 100 * 1024 * 1024;

pub(super) const PASTE_START_MARKER: &[u8] = b"\x1b[200~";
pub(super) const PASTE_END_MARKER: &[u8] = b"\x1b[201~";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingPartial {
    None,
    LoneEsc,
    ReplyInProgress,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum MarkerMatch {
    Complete,
    Prefix,
    No,
}

pub(super) fn marker_match(buf: &[u8], marker: &[u8]) -> MarkerMatch {
    if buf.len() >= marker.len() {
        if &buf[..marker.len()] == marker {
            MarkerMatch::Complete
        } else {
            MarkerMatch::No
        }
    } else if marker.starts_with(buf) {
        MarkerMatch::Prefix
    } else {
        MarkerMatch::No
    }
}

/// Outcome of a single OSC/CSI walk over a byte buffer. Distinguishing
/// "needs more bytes" from "malformed" is what lets the residue
/// scrubber buffer partial sequences across `feed()` calls instead of
/// leaking their bytes into keyboard residue.
#[derive(Debug, Clone, Copy)]
pub(super) enum SeqStatus {
    /// Sequence is complete; consume `len` bytes from the head of buf.
    Complete(usize),
    /// Sequence is a valid prefix; caller should buffer these bytes
    /// and prepend them to the next chunk.
    NeedMore,
    /// Sequence is malformed (bare ESC mid-payload, non-whitelisted
    /// final byte, length cap hit). Caller should fall through to
    /// emitting the leading byte as residue.
    Malformed,
}

pub(super) fn osc_status(buf: &[u8]) -> SeqStatus {
    if buf.first() != Some(&0x1b) || buf.get(1) != Some(&b']') {
        return SeqStatus::Malformed;
    }
    let mut i = 2;
    while i < buf.len() {
        match buf[i] {
            0x07 => return SeqStatus::Complete(i + 1),
            0x1b => match buf.get(i + 1) {
                Some(&b'\\') => return SeqStatus::Complete(i + 2),
                // Bare ESC followed by something other than `\` —
                // malformed under the ST-only termination we accept.
                Some(_) => return SeqStatus::Malformed,
                // ESC at the very tail; the next chunk may bring `\`
                // and finish the sequence.
                None => return SeqStatus::NeedMore,
            },
            _ => i += 1,
        }
    }
    SeqStatus::NeedMore
}

pub(super) fn apc_status(buf: &[u8]) -> SeqStatus {
    if buf.first() != Some(&0x1b) || buf.get(1) != Some(&b'_') {
        return SeqStatus::Malformed;
    }
    let mut i = 2;
    while i < buf.len() {
        match buf[i] {
            0x1b => match buf.get(i + 1) {
                Some(&b'\\') => return SeqStatus::Complete(i + 2),
                Some(_) => return SeqStatus::Malformed,
                None => return SeqStatus::NeedMore,
            },
            _ => i += 1,
        }
    }
    SeqStatus::NeedMore
}

pub(super) fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Walk a whitelisted CSI report starting at the head of `buf`.
pub(super) fn csi_status(buf: &[u8]) -> SeqStatus {
    if buf.first() != Some(&0x1b) || buf.get(1) != Some(&b'[') {
        return SeqStatus::Malformed;
    }
    let mut i = 2;
    let max = 256;
    while i < buf.len() && i < max {
        let b = buf[i];
        match b {
            0x20..=0x3F => i += 1,
            b't' | b'y' | b'c' | b'n' => return SeqStatus::Complete(i + 1),
            0x40..=0x7E => return SeqStatus::Malformed, // non-whitelisted final
            _ => return SeqStatus::Malformed,
        }
    }
    if i >= max {
        // CSI ran past the cap without terminating — treat as malformed
        // so the leading byte falls through to residue and parsing
        // resumes from the next position.
        SeqStatus::Malformed
    } else {
        SeqStatus::NeedMore
    }
}

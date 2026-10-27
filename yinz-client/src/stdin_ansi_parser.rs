//! Continuous, parser for host-terminal replies arriving on
//! stdin.
//!
//! This parser routes stdin bytes through a private `termwiz::InputParser`,
//! classifies OSC / CSI-report events into `HostReply` variants, and lets
//! all other bytes (keyboard input) pass through as a residue byte sequence
//! that the caller feeds to the normal keyboard parser.

mod forward_timeout;
mod host_reply;
mod seq_scan;

pub use forward_timeout::{forward_timeout_runtime, schedule_timeout};
pub use host_reply::{AnsiStdinInstruction, HostReply};
pub use seq_scan::PendingPartial;

use seq_scan::{
    apc_status, contains_subslice, csi_status, marker_match, osc_status, MarkerMatch, SeqStatus,
    PARTIAL_BUFFER_CAP_BYTES, PASTE_END_MARKER, PASTE_START_MARKER,
};
use serde::{Deserialize, Serialize};
use yinz_ipc::nested_session;
use yinz_key_codec::vendored::termwiz::input::{InputEvent, InputParser};

/// Describe the terminal implementation of synchronised output
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SyncOutput {
    Dcs,
    Csi,
}

impl SyncOutput {
    pub fn start_seq(&self) -> &'static [u8] {
        static CSI_BSU_SEQ: &[u8] = "\u{1b}[?2026h".as_bytes();
        static DCS_BSU_SEQ: &[u8] = "\u{1b}P=1s\u{1b}".as_bytes();
        match self {
            SyncOutput::Dcs => DCS_BSU_SEQ,
            SyncOutput::Csi => CSI_BSU_SEQ,
        }
    }

    pub fn end_seq(&self) -> &'static [u8] {
        static CSI_ESU_SEQ: &[u8] = "\u{1b}[?2026l".as_bytes();
        static DCS_ESU_SEQ: &[u8] = "\u{1b}P=2s\u{1b}".as_bytes();
        match self {
            SyncOutput::Dcs => DCS_ESU_SEQ,
            SyncOutput::Csi => CSI_ESU_SEQ,
        }
    }
}

/// The "slot" tracking state for a single forwarded query currently in
/// flight to the host terminal. The parser accumulates raw reply bytes
/// into `reply_bytes` until it sees a Primary-DA (`c`) reply, which acts
/// as the serializing barrier. The timer that enforces the 500 ms
/// deadline lives on the forward-timeout runtime and owns its own
/// wall-clock — the parser itself is deadline-agnostic.
#[derive(Debug, Clone)]
pub struct ForwardSlot {
    pub token: u32,
    pub reply_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ClipboardForwardSlot {
    pub token: u32,
}

pub const CLIENT_CLIPBOARD_FORWARD_TIMEOUT_MS: u64 = 30_000;
pub const CLIENT_BARRIER_FORWARD_TIMEOUT_MS: u64 = 500;

/// How long the startup kitty-graphics probe waits before reporting "no graphics".
///
/// Longer than the barrier forward timeout on purpose: this deadline is crossed once per client
/// startup, a false negative costs a picture rather than correctness, and a high-latency link
/// should not lose its images to an eager deadline. Nothing blocks on it - the answer starts
/// `false` everywhere, so this only ever brings a *negative* forward in time for a transport that
/// answers neither the probe nor the Primary-DA barrier that backstops it.
pub const CLIENT_KITTY_PROBE_TIMEOUT_MS: u64 = 2_000;

/// Which of the two forwarding mechanisms a query uses.
///
/// The client can hold one window open per kind at a time, and they close
/// on different signals, so the slots themselves are separate. What they do
/// share is the orchestration around them — take over any stale slot, open
/// a new one, arm a deadline, write the query — which is what this enum
/// parameterises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardKind {
    /// Closed by the Primary-DA reply acting as a serializing barrier,
    /// accumulating every reply seen up to that point.
    Barrier,
    /// Closed by a single OSC 52 reply, or by any user input (the host
    /// evidently isn't going to answer). Carries no accumulated bytes.
    Clipboard,
}

impl ForwardKind {
    /// How long to wait before giving up on the host and releasing the
    /// server's in-flight slot.
    pub fn timeout(&self) -> std::time::Duration {
        let ms = match self {
            ForwardKind::Barrier => CLIENT_BARRIER_FORWARD_TIMEOUT_MS,
            ForwardKind::Clipboard => CLIENT_CLIPBOARD_FORWARD_TIMEOUT_MS,
        };
        std::time::Duration::from_millis(ms)
    }

    /// Bytes appended to the query to provoke the closing reply. The
    /// clipboard kind is closed by the OSC 52 answer itself, so it needs
    /// nothing extra.
    pub fn barrier_suffix(&self) -> &'static [u8] {
        match self {
            ForwardKind::Barrier => b"\x1b[c",
            ForwardKind::Clipboard => b"",
        }
    }

    /// Human-readable name, used when reporting a slot we had to take over.
    pub fn label(&self) -> &'static str {
        match self {
            ForwardKind::Barrier => "forward",
            ForwardKind::Clipboard => "clipboard forward",
        }
    }
}

/// Return value of `feed()`.
#[derive(Debug, Clone, Default)]
pub struct ParseOutput {
    /// Classified host replies (zero or more).
    pub replies: Vec<HostReply>,
    /// A completed forwarded reply (Primary-DA barrier seen), ready to be
    /// sent to the server. At most one per feed call; more than one in a
    /// single feed would indicate the host emitted two barriers, in which
    /// case only the first is honored.
    pub completed_forward: Option<(u32, Vec<u8>)>,
    pub completed_clipboard_forward: Option<(u32, Vec<u8>)>,
    /// OSC 99 notification-response payloads (one per OSC 99 found in
    /// the chunk). Routed by the caller as
    /// `InputInstruction::DesktopNotificationResponse`. Lives here, not
    /// in the keyboard-parser path, because the residue scrubber
    /// strips all OSC bytes before the keyboard parser sees them.
    pub desktop_notifications: Vec<Vec<u8>>,
    /// Residue bytes that were not classified as host replies. These are
    /// the bytes the caller should feed to the keyboard parser.
    pub residue: Vec<u8>,
    pub nested_frames: Vec<Vec<u8>>,
    /// `true` when the parser still holds buffered partial OSC/CSI
    /// bytes that need an idle-flush to release. The caller uses this
    /// to schedule a finalize tick even when the current chunk
    /// produced no residue (so a lone trailing ESC isn't stranded
    /// indefinitely). See `StdinAnsiParser::finalize`.
    pub has_partial_state: bool,
}

/// Hold an incomplete sequence over until the next chunk arrives.
///
/// Past `PARTIAL_BUFFER_CAP_BYTES` we stop believing a terminator is coming
/// and flush the bytes to residue instead, so a runaway sequence is never
/// silently swallowed. Returns true when the limit was exceeded.
fn buffer_partial(slot: &mut Vec<u8>, tail: Vec<u8>, out: &mut Vec<u8>, limit: usize) -> bool {
    if tail.len() > limit {
        out.extend_from_slice(&tail);
        true
    } else {
        *slot = tail;
        false
    }
}

fn is_user_input(event: &InputEvent) -> bool {
    match event {
        InputEvent::Key(_) | InputEvent::Paste(_) => true,
        InputEvent::Mouse(mouse_event) => !mouse_event.mouse_buttons.is_empty(),
        InputEvent::PixelMouse(mouse_event) => !mouse_event.mouse_buttons.is_empty(),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupKittyProbe {
    NotSent,
    AwaitingReply,
    /// The barrier or deadline reported `false`, but the graphics reply is still pending:
    /// a late `i=31;OK` can turn support on, and an explicit refusal settles the probe.
    UnsupportedAwaitingReply,
    Resolved,
}

/// A paste crossing a recovery boundary must never reach the new keyboard parser.
/// Keep only the matched marker length, even for arbitrarily long pastes.
enum RecoveryPaste {
    Opening(usize),
    Closing(usize),
}

/// Continuous host-reply parser. Lives for the whole client session.
pub struct StdinAnsiParser {
    inner: InputParser,
    partial_buffer_limit: usize,
    /// Active forwarding slot: `Some` while a forwarded query is in
    /// flight, `None` otherwise.
    active_forward: Option<ForwardSlot>,
    active_clipboard_forward: Option<ClipboardForwardSlot>,
    /// Bytes of an OSC sequence whose terminator hasn't arrived yet.
    /// Carried across feed() calls so the next chunk can complete it.
    partial_osc: Vec<u8>,
    /// Same for CSI device-control reports.
    partial_csi: Vec<u8>,
    partial_paste: Vec<u8>,
    nested_frame_extractor: nested_session::NestedFrameExtractor,
    in_bracketed_paste: bool,
    recovery_paste: Option<RecoveryPaste>,
    startup_kitty_probe: StartupKittyProbe,
    partial_apc: Vec<u8>,
}

impl std::fmt::Debug for StdinAnsiParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdinAnsiParser")
            .field("active_forward", &self.active_forward)
            .field("partial_osc_len", &self.partial_osc.len())
            .field("partial_csi_len", &self.partial_csi.len())
            .finish()
    }
}

impl StdinAnsiParser {
    pub fn new() -> Self {
        StdinAnsiParser {
            inner: InputParser::new(),
            partial_buffer_limit: PARTIAL_BUFFER_CAP_BYTES,
            active_forward: None,
            active_clipboard_forward: None,
            partial_osc: Vec::new(),
            partial_csi: Vec::new(),
            partial_paste: Vec::new(),
            nested_frame_extractor: nested_session::NestedFrameExtractor::new(),
            in_bracketed_paste: false,
            recovery_paste: None,
            startup_kitty_probe: StartupKittyProbe::NotSent,
            partial_apc: Vec::new(),
        }
    }

    pub fn expect_kitty_probe_reply(&mut self) {
        self.startup_kitty_probe = StartupKittyProbe::AwaitingReply;
    }

    /// The startup probe's deadline fired. Report "no graphics" now so a transport that answers
    /// neither the probe nor the Primary-DA barrier - mosh's emulator swallows the APC, and a
    /// proxy may swallow both - does not leave every client stuck on "unknown" forever.
    ///
    /// Stays open to a late `i=31;OK` afterwards, which is why this does not go straight to
    /// `Resolved`. Returns `None` once a reply settles the probe or a barrier or earlier call
    /// reports the provisional negative, so the caller needs no cancellation of its own.
    pub fn resolve_kitty_probe_by_timeout(&mut self) -> Option<HostReply> {
        if self.startup_kitty_probe != StartupKittyProbe::AwaitingReply {
            return None;
        }
        self.startup_kitty_probe = StartupKittyProbe::UnsupportedAwaitingReply;
        Some(HostReply::KittyGraphicsSupport(false))
    }

    /// Open a forwarding window for `token`. Subsequent reply events that
    /// arrive before the Primary-DA barrier will be accumulated into the
    /// slot's `reply_bytes`, in addition to being dispatched as normal
    /// classified `HostReply` events.
    ///
    /// The server serializes forwarded queries globally (`forward_in_flight`
    /// on `Screen`), but its own backstop timeout can release that slot and
    /// dispatch the next query before this client's per-slot timer has run,
    /// so callers hand a still-open slot over with `take_active_forward`
    /// first. The guards below catch a caller that skipped that handover:
    /// debug builds panic so bugs surface during testing, release builds
    /// log and clobber the previous slot (whose accumulated bytes would
    /// otherwise silently leak).
    pub fn open_forward(&mut self, token: u32) {
        debug_assert!(
            self.active_forward.is_none(),
            "open_forward({}) called while slot for token {:?} is still active",
            token,
            self.active_forward.as_ref().map(|s| s.token),
        );
        if let Some(existing) = self.active_forward.as_ref() {
            log::warn!(
                "open_forward({}) re-entered with existing slot token={} ({} accumulated bytes \
                 will be dropped); server serialization should have prevented this",
                token,
                existing.token,
                existing.reply_bytes.len(),
            );
        }
        self.active_forward = Some(ForwardSlot {
            token,
            reply_bytes: Vec::new(),
        });
    }

    /// Close an active forwarding window without a barrier (timeout path).
    /// Returns the accumulated reply bytes and the token, if any.
    pub fn close_forward_on_timeout(&mut self, token: u32) -> Option<(u32, Vec<u8>)> {
        match &self.active_forward {
            Some(slot) if slot.token == token => {
                let slot = self.active_forward.take().unwrap();
                Some((slot.token, slot.reply_bytes))
            },
            _ => None,
        }
    }

    /// Close whatever forwarding window is currently open, whichever token
    /// it belongs to, and return its token together with the bytes it
    /// accumulated. Used to hand a slot the server has already given up on
    /// over to the forward that replaced it, instead of clobbering it.
    pub fn take_active_forward(&mut self) -> Option<(u32, Vec<u8>)> {
        self.active_forward
            .take()
            .map(|slot| (slot.token, slot.reply_bytes))
    }

    /// Open a forwarding window of `kind` for `token`.
    pub fn open(&mut self, kind: ForwardKind, token: u32) {
        match kind {
            ForwardKind::Barrier => self.open_forward(token),
            ForwardKind::Clipboard => self.open_clipboard_forward(token),
        }
    }

    /// Close the `kind` window if it is still open for `token`, returning
    /// whatever it accumulated. Token-guarded, so a timer that fires after
    /// the window already closed finds nothing and does nothing.
    pub fn close_on_timeout(&mut self, kind: ForwardKind, token: u32) -> Option<(u32, Vec<u8>)> {
        match kind {
            ForwardKind::Barrier => self.close_forward_on_timeout(token),
            ForwardKind::Clipboard => self.close_clipboard_forward_on_timeout(token),
        }
    }

    /// Close whatever `kind` window is open, whichever token it belongs to.
    pub fn take_active(&mut self, kind: ForwardKind) -> Option<(u32, Vec<u8>)> {
        match kind {
            ForwardKind::Barrier => self.take_active_forward(),
            ForwardKind::Clipboard => self.take_active_clipboard_forward(),
        }
    }

    pub fn open_clipboard_forward(&mut self, token: u32) {
        debug_assert!(
            self.active_clipboard_forward.is_none(),
            "open_clipboard_forward({}) called while slot for token {:?} is still active",
            token,
            self.active_clipboard_forward.as_ref().map(|s| s.token),
        );
        self.active_clipboard_forward = Some(ClipboardForwardSlot { token });
    }

    pub fn close_clipboard_forward_on_timeout(&mut self, token: u32) -> Option<(u32, Vec<u8>)> {
        match &self.active_clipboard_forward {
            Some(slot) if slot.token == token => {
                let slot = self.active_clipboard_forward.take().unwrap();
                Some((slot.token, Vec::new()))
            },
            _ => None,
        }
    }

    pub fn take_active_clipboard_forward(&mut self) -> Option<(u32, Vec<u8>)> {
        self.active_clipboard_forward
            .take()
            .map(|slot| (slot.token, Vec::new()))
    }

    #[cfg(test)]
    pub fn active_clipboard_forward_token(&self) -> Option<u32> {
        self.active_clipboard_forward.as_ref().map(|s| s.token)
    }

    /// Currently-open slot's token, if any. Test-only inspector;
    /// production code drives slot lifecycle through `open_forward`,
    /// `close_forward_on_timeout`, and `feed()` directly.
    #[cfg(test)]
    pub fn active_forward_token(&self) -> Option<u32> {
        self.active_forward.as_ref().map(|s| s.token)
    }

    /// Consume a chunk of raw stdin bytes. Returns classified host replies
    /// (to be dispatched to the server's cached-state consumers), at most
    /// one completed forwarded reply (barrier closed the window), and the
    /// residue bytes that were not part of any classified sequence — these
    /// are the bytes the caller should feed to the keyboard parser.
    pub fn feed(&mut self, bytes: &[u8]) -> ParseOutput {
        let bytes = self.discard_recovery_paste(bytes);
        let mut out = ParseOutput::default();
        let (bytes, nested_frames) = self.nested_frame_extractor.extract(bytes);
        let bytes = &bytes[..];
        out.nested_frames = nested_frames;
        let sanitized = self.extract_kitty_probe_reply(bytes, &mut out.replies);
        // Collect events first (borrow-splits the InputParser across the
        // callback and the post-processing mutations).
        let mut events = Vec::new();
        let mut residue = Vec::new();
        self.inner.parse(
            &sanitized,
            |event| {
                events.push(event);
            },
            true, // maybe_more — typical stream usage
        );
        for event in events {
            match event {
                InputEvent::OperatingSystemCommand(payload) => {
                    // OSC 99 (desktop-notification response) is routed
                    // here rather than from the keyboard parser because
                    // the residue scrubber removes all OSC bytes before
                    // the keyboard parser runs. Other OSCs are
                    // classified into `HostReply` for cached-state
                    // refinement.
                    if payload.starts_with(b"99;") {
                        out.desktop_notifications
                            .push(payload.get(3..).unwrap_or_default().to_vec());
                        // The response is consumed here; it must not also
                        // land in an open forwarding slot, where the pane
                        // that asked for some other reply would see it.
                        continue;
                    } else if let Some(reply) = HostReply::from_osc_payload(&payload) {
                        out.replies.push(reply);
                    }
                    if payload.starts_with(b"52;")
                        && let Some(slot) = self.active_clipboard_forward.take()
                    {
                        let mut bytes = b"\x1b]".to_vec();
                        bytes.extend_from_slice(&payload);
                        bytes.extend_from_slice(b"\x1b\\");
                        out.completed_clipboard_forward = Some((slot.token, bytes));
                        continue;
                    }
                    if let Some(slot) = self.active_forward.as_mut() {
                        // Re-serialize so the pane's pty sees a legal OSC.
                        // Terminators vary by host; ST (ESC \) is always safe.
                        slot.reply_bytes.extend_from_slice(b"\x1b]");
                        slot.reply_bytes.extend_from_slice(&payload);
                        slot.reply_bytes.extend_from_slice(b"\x1b\\");
                    }
                },
                InputEvent::DeviceControlReply {
                    params,
                    final_byte,
                    raw,
                    ..
                } => {
                    match final_byte {
                        b'c' => {
                            // The graphics reply can arrive after this barrier. Report the
                            // provisional negative once, and keep waiting for that reply.
                            if self.startup_kitty_probe == StartupKittyProbe::AwaitingReply {
                                self.startup_kitty_probe =
                                    StartupKittyProbe::UnsupportedAwaitingReply;
                                out.replies.push(HostReply::KittyGraphicsSupport(false));
                            }
                            if let Some(reply) = HostReply::sixel_support_from_primary_da(&raw) {
                                out.replies.push(reply);
                            }
                            // Primary-DA — the barrier. Close the slot and
                            // emit the completed forwarded reply if active.
                            if let Some(slot) = self.active_forward.take() {
                                out.completed_forward = Some((slot.token, slot.reply_bytes));
                            }
                            // Primary-DA is NOT double-dispatched — it has
                            // no cached-state counterpart.
                        },
                        _ => {
                            if let Some(reply) = HostReply::from_csi_report(&raw) {
                                out.replies.push(reply);
                            }
                            if let Some(slot) = self.active_forward.as_mut() {
                                slot.reply_bytes.extend_from_slice(&raw);
                            }
                            // Suppress unused-variable warning for params.
                            let _ = params;
                        },
                    }
                },
                // Everything else is keyboard / mouse / paste / wake input;
                // we need those bytes to reach the keyboard parser. We can
                // not reconstruct the exact bytes from a parsed event here,
                // so we rely on the caller's own second pass through the
                // keyboard parser: the residue is the concatenation of all
                // input bytes that are NOT part of a classified reply. To
                // produce that residue deterministically, we re-scan the
                // buffer a second time below.
                other => {
                    if self.active_clipboard_forward.is_some()
                        && is_user_input(&other)
                        && out.completed_clipboard_forward.is_none()
                        && let Some(slot) = self.active_clipboard_forward.take()
                    {
                        out.completed_clipboard_forward = Some((slot.token, Vec::new()));
                    }
                },
            }
        }
        // Produce the residue: replay the input through a scratch parser
        // that strips out OSC payloads and whitelisted CSI reports. All
        // other bytes pass through unchanged. The walk is stateful so
        // an OSC/CSI sequence split across `feed()` calls is buffered
        // rather than leaking into residue.
        let (stripped, overflowed) = self.strip_replies(&sanitized);
        residue.extend(stripped);
        if overflowed {
            // The decoder also buffers unfinished replies. Abandon both copies together.
            self.inner = InputParser::new();
        }
        out.residue = residue;
        out.has_partial_state = !self.partial_osc.is_empty()
            || !self.partial_csi.is_empty()
            || !self.partial_paste.is_empty()
            || !self.nested_frame_extractor.partial_bytes().is_empty()
            || !self.partial_apc.is_empty();
        out
    }

    pub fn pending_partial(&self) -> PendingPartial {
        if !self.partial_paste.is_empty() || !self.partial_apc.is_empty() {
            PendingPartial::ReplyInProgress
        } else if self.partial_csi.is_empty()
            && self.partial_osc.is_empty()
            && self.nested_frame_extractor.partial_bytes() == [0x1b]
        {
            PendingPartial::LoneEsc
        } else if !self.nested_frame_extractor.partial_bytes().is_empty() {
            PendingPartial::ReplyInProgress
        } else if self.partial_csi.is_empty() && self.partial_osc.is_empty() {
            PendingPartial::None
        } else if self.partial_csi.is_empty() && self.partial_osc == [0x1b] {
            PendingPartial::LoneEsc
        } else if (self.partial_csi.is_empty() && self.partial_osc == [0x1b, b']'])
            || (self.partial_osc.is_empty() && self.partial_csi == [0x1b, b'['])
        {
            PendingPartial::BareIntroducer
        } else {
            PendingPartial::ReplyInProgress
        }
    }

    /// Drains a partial that should not wait for the reply flush guard: a lone
    /// ESC or a bare `ESC [` / `ESC ]` introducer, from whichever buffer holds it.
    /// The nested-frame extractor keeps priority, as in `pending_partial`.
    pub fn finalize_fast_partial(&mut self) -> Vec<u8> {
        if self.in_bracketed_paste {
            return Vec::new();
        }
        match self.pending_partial() {
            PendingPartial::LoneEsc | PendingPartial::BareIntroducer => {
                if !self.nested_frame_extractor.partial_bytes().is_empty() {
                    self.nested_frame_extractor.take_partial()
                } else if !self.partial_osc.is_empty() {
                    std::mem::take(&mut self.partial_osc)
                } else {
                    std::mem::take(&mut self.partial_csi)
                }
            },
            PendingPartial::None | PendingPartial::ReplyInProgress => Vec::new(),
        }
    }

    pub fn finalize_force(&mut self) -> Vec<u8> {
        // An idle timeout cannot end a paste: its closing marker may still be in flight.
        if self.in_bracketed_paste {
            return Vec::new();
        }
        let mut partial_nested_frame = self.nested_frame_extractor.take_partial();
        let mut out = Vec::with_capacity(
            self.partial_osc.len()
                + self.partial_csi.len()
                + self.partial_paste.len()
                + partial_nested_frame.len()
                + self.partial_apc.len(),
        );
        out.append(&mut self.partial_osc);
        out.append(&mut self.partial_csi);
        out.append(&mut self.partial_paste);
        out.append(&mut partial_nested_frame);
        out.append(&mut self.partial_apc);
        out
    }

    pub(crate) fn reset_for_recovery(&mut self) {
        let in_paste = self.in_bracketed_paste;
        self.in_bracketed_paste = false;
        let pending = self.finalize_force();
        self.inner = InputParser::new();
        // Further transitions must not forget an already quarantined paste.
        if self.recovery_paste.is_none() {
            if in_paste {
                let matched = (1..PASTE_END_MARKER.len())
                    .rev()
                    .find(|&len| pending.ends_with(&PASTE_END_MARKER[..len]))
                    .unwrap_or(0);
                self.recovery_paste = Some(RecoveryPaste::Closing(matched));
            } else if !pending.is_empty() && PASTE_START_MARKER.starts_with(&pending) {
                self.recovery_paste = Some(RecoveryPaste::Opening(pending.len()));
            }
        }
    }

    fn discard_recovery_paste<'a>(&mut self, bytes: &'a [u8]) -> &'a [u8] {
        for (i, &byte) in bytes.iter().enumerate() {
            match self.recovery_paste.as_mut() {
                None => return &bytes[i..],
                Some(RecoveryPaste::Opening(matched)) => {
                    if byte != PASTE_START_MARKER[*matched] {
                        // The old prefix was not a paste. Only new bytes may become input.
                        self.recovery_paste = None;
                        return &bytes[i..];
                    }
                    *matched += 1;
                    if *matched == PASTE_START_MARKER.len() {
                        self.recovery_paste = Some(RecoveryPaste::Closing(0));
                    }
                },
                Some(RecoveryPaste::Closing(matched)) => {
                    if byte == PASTE_END_MARKER[*matched] {
                        *matched += 1;
                        if *matched == PASTE_END_MARKER.len() {
                            self.recovery_paste = None;
                        }
                    } else {
                        *matched = usize::from(byte == PASTE_END_MARKER[0]);
                    }
                },
            }
        }
        &bytes[bytes.len()..]
    }

    fn extract_kitty_probe_reply(&mut self, bytes: &[u8], replies: &mut Vec<HostReply>) -> Vec<u8> {
        let mut working = std::mem::take(&mut self.partial_apc);
        if working.is_empty() && self.partial_osc == [0x1b] && bytes.first() == Some(&b'_') {
            working.append(&mut self.partial_osc);
        }
        working.extend_from_slice(bytes);
        let mut out = Vec::with_capacity(working.len());
        let mut i = 0;
        while i < working.len() {
            let rest = &working[i..];
            if rest.len() >= 2 && rest[0] == 0x1b && rest[1] == b'_' {
                match rest.get(2) {
                    Some(&b'G') => {},
                    Some(_) => {
                        out.push(working[i]);
                        i += 1;
                        continue;
                    },
                    None => {
                        self.partial_apc = rest.to_vec();
                        return out;
                    },
                }
                match apc_status(rest) {
                    SeqStatus::Complete(len) => {
                        let payload = &rest[2..len - 2];
                        if matches!(
                            self.startup_kitty_probe,
                            StartupKittyProbe::AwaitingReply
                                | StartupKittyProbe::UnsupportedAwaitingReply
                        ) && payload.first() == Some(&b'G')
                            && contains_subslice(payload, b"i=31")
                        {
                            let supported = contains_subslice(payload, b"i=31;OK");
                            // A late `OK` still turns the capability on - a slow host is not an
                            // incapable one - but a late refusal only repeats what the barrier
                            // or deadline said, and a repeat costs the server a whole `ModeUpdate`
                            // fan-out to every plugin for no change.
                            if supported
                                || self.startup_kitty_probe == StartupKittyProbe::AwaitingReply
                            {
                                replies.push(HostReply::KittyGraphicsSupport(supported));
                            }
                            self.startup_kitty_probe = StartupKittyProbe::Resolved;
                        }
                        i += len;
                        continue;
                    },
                    SeqStatus::NeedMore => {
                        let tail = rest.to_vec();
                        if tail.len() > self.partial_buffer_limit {
                            out.extend_from_slice(&tail);
                        } else {
                            self.partial_apc = tail;
                        }
                        return out;
                    },
                    SeqStatus::Malformed => {
                        out.push(working[i]);
                        i += 1;
                        continue;
                    },
                }
            }
            out.push(working[i]);
            i += 1;
        }
        out
    }

    /// Walk `bytes` (with any pending partial buffer prepended) and drop
    /// any OSC/whitelisted-CSI sequences, returning the remaining bytes
    /// verbatim (keyboard residue). This is a byte-level scrubber — it
    /// does not produce events, only bytes and whether an OSC/CSI partial overflowed.
    ///
    /// If the chunk ends mid-sequence, the unterminated tail is held in
    /// `self.partial_osc` or `self.partial_csi` and prepended to the
    /// next call's input — so the corresponding bytes never reach
    /// residue (and never appear as spurious keypresses) while waiting
    /// for the rest of the sequence.
    fn strip_replies(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        // Prepend any pending partial. At most one of (partial_osc,
        // partial_csi) is non-empty at any time — the previous walk
        // either completed all sequences or stopped at exactly one
        // unterminated tail.
        let mut working: Vec<u8> = Vec::with_capacity(
            self.partial_osc.len()
                + self.partial_csi.len()
                + self.partial_paste.len()
                + bytes.len(),
        );
        working.append(&mut self.partial_osc);
        working.append(&mut self.partial_csi);
        working.append(&mut self.partial_paste);
        working.extend_from_slice(bytes);

        let mut out = Vec::with_capacity(working.len());
        let mut i = 0;
        while i < working.len() {
            let rest = &working[i..];
            if self.in_bracketed_paste {
                if rest[0] == 0x1b {
                    match marker_match(rest, PASTE_END_MARKER) {
                        MarkerMatch::Complete => {
                            out.extend_from_slice(PASTE_END_MARKER);
                            self.in_bracketed_paste = false;
                            i += PASTE_END_MARKER.len();
                            continue;
                        },
                        MarkerMatch::Prefix => {
                            buffer_partial(
                                &mut self.partial_paste,
                                rest.to_vec(),
                                &mut out,
                                self.partial_buffer_limit,
                            );
                            return (out, false);
                        },
                        MarkerMatch::No => {},
                    }
                }
                out.push(working[i]);
                i += 1;
                continue;
            }
            if rest.len() >= 2
                && rest[0] == 0x1b
                && rest[1] == b'['
                && let MarkerMatch::Complete = marker_match(rest, PASTE_START_MARKER)
            {
                out.extend_from_slice(PASTE_START_MARKER);
                self.in_bracketed_paste = true;
                i += PASTE_START_MARKER.len();
                continue;
            }
            // OSC: ESC ] ... (BEL | ESC \)
            if rest.len() >= 2 && rest[0] == 0x1b && rest[1] == b']' {
                match osc_status(rest) {
                    SeqStatus::Complete(len) => {
                        i += len;
                        continue;
                    },
                    SeqStatus::NeedMore => {
                        let overflowed = buffer_partial(
                            &mut self.partial_osc,
                            rest.to_vec(),
                            &mut out,
                            self.partial_buffer_limit,
                        );
                        return (out, overflowed);
                    },
                    SeqStatus::Malformed => {
                        out.push(working[i]);
                        i += 1;
                        continue;
                    },
                }
            }
            // Whitelisted CSI report: ESC [ <params>* <intermediates>* <final>
            if rest.len() >= 2 && rest[0] == 0x1b && rest[1] == b'[' {
                match csi_status(rest) {
                    SeqStatus::Complete(len) => {
                        i += len;
                        continue;
                    },
                    SeqStatus::NeedMore => {
                        let overflowed = buffer_partial(
                            &mut self.partial_csi,
                            rest.to_vec(),
                            &mut out,
                            self.partial_buffer_limit,
                        );
                        return (out, overflowed);
                    },
                    SeqStatus::Malformed => {
                        out.push(working[i]);
                        i += 1;
                        continue;
                    },
                }
            }
            // Lone trailing ESC at the tail — could be the start of
            // either OSC or CSI; the next byte will disambiguate. Buffer
            // it under partial_osc by convention; the next call's
            // walker re-routes based on the actual second byte.
            if rest.len() == 1 && rest[0] == 0x1b {
                self.partial_osc = vec![0x1b];
                return (out, false);
            }
            out.push(working[i]);
            i += 1;
        }
        (out, false)
    }
}

#[cfg(test)]
#[path = "./unit/stdin_ansi_parser_tests.rs"]
mod tests;

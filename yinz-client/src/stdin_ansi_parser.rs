//! Continuous, parser for host-terminal replies arriving on
//! stdin.
//!
//! This parser routes stdin bytes through a private `termwiz::InputParser`,
//! classifies OSC / CSI-report events into `HostReply` variants, and lets
//! all other bytes (keyboard input) pass through as a residue byte sequence
//! that the caller feeds to the normal keyboard parser.

mod color_prefix;
mod forward_timeout;
mod host_reply;
mod seq_scan;

pub use forward_timeout::{forward_timeout_runtime, schedule_timeout};
pub use host_reply::{AnsiStdinInstruction, HostReply};
pub use seq_scan::PendingPartial;

use color_prefix::ColorPrefix;
use seq_scan::{
    apc_status, contains_subslice, csi_status, marker_match, osc_status, MarkerMatch, SeqStatus,
    PARTIAL_BUFFER_CAP_BYTES, PASTE_END_MARKER, PASTE_START_MARKER,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
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

/// Input that may still complete a terminal sequence across a recovery boundary.
enum RecoveryPartial {
    Prefix {
        bytes: Vec<u8>,
        fresh_at: usize,
        boundary: InputBoundary,
    },
    Paste(usize),
    Osc(PartialOsc),
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum InputBoundary {
    Idle,
    FirstRepaint,
    Recovery,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum OscKind {
    HostColor {
        prefix: ColorPrefix,
        complete_at_boundary: bool,
    },
    InterruptedColor {
        complete: bool,
    },
    Clipboard,
    RetiredClipboard,
}

/// Bytes before `fresh_at` already belonged to the terminal sequence. Only
/// subsequent bytes can be keyboard input if the terminal abandons the reply.
struct PartialOsc {
    bytes: Vec<u8>,
    fresh_at: usize,
    kind: OscKind,
}

impl PartialOsc {
    fn new(bytes: Vec<u8>, kind: OscKind) -> Self {
        let mut partial = Self {
            bytes,
            fresh_at: 0,
            kind,
        };
        partial.mark_boundary();
        partial
    }

    fn is_clipboard(&self) -> bool {
        matches!(self.kind, OscKind::Clipboard | OscKind::RetiredClipboard)
    }

    /// A complete color may be waiting only for BEL or ST while the user types.
    /// Send distinguishable keys through normal input decoding and keep the
    /// report for its real terminator. ESC is handled by the framing code.
    fn consume_color_byte(&mut self, byte: u8, keyboard: &mut Vec<u8>) -> bool {
        match self.kind {
            OscKind::HostColor {
                prefix,
                mut complete_at_boundary,
            } => {
                if let Some(next) = prefix.advance(byte) {
                    self.bytes.push(byte);
                    if !complete_at_boundary && next.complete() {
                        complete_at_boundary = true;
                        self.fresh_at = self.bytes.len();
                    }
                    self.kind = OscKind::HostColor {
                        prefix: next,
                        complete_at_boundary,
                    };
                } else {
                    keyboard.extend_from_slice(&self.bytes[self.fresh_at..]);
                    self.bytes.truncate(self.fresh_at);
                    self.kind = OscKind::InterruptedColor {
                        complete: complete_at_boundary,
                    };
                    keyboard.push(byte);
                }
            },
            OscKind::InterruptedColor { .. } => keyboard.push(byte),
            _ => return false,
        }
        true
    }

    fn accepts(&self, byte: u8) -> bool {
        byte == 0x1b
            || if self.is_clipboard() {
                byte.is_ascii_alphanumeric() || b";+/=?".contains(&byte)
            } else {
                (0x20..=0x7e).contains(&byte)
            }
    }

    fn keyboard_suffix(&mut self) -> Vec<u8> {
        // Base64 is ambiguous. Replay only a new escape that broke the clipboard
        // sequence. Other OSCs use the ordinary fallback for a malformed reply.
        let start = if self.is_clipboard() {
            let fresh_escape = self.bytes[self.fresh_at..].ends_with(b"\x1b");
            self.bytes.len() - usize::from(fresh_escape)
        } else {
            self.fresh_at
        };
        self.bytes.split_off(start)
    }

    fn mark_boundary(&mut self) {
        // A retired reply needs only its identity and a possible first byte of ST.
        if self.kind == OscKind::RetiredClipboard {
            let escaped = self.bytes.ends_with(b"\x1b");
            self.bytes.truncate(b"\x1b]52;".len());
            if escaped {
                self.bytes.push(0x1b);
            }
        }
        if let OscKind::HostColor {
            prefix,
            complete_at_boundary,
        } = &mut self.kind
        {
            *complete_at_boundary = prefix.complete();
        }
        self.fresh_at = self.bytes.len();
    }
}

const HOST_COLOR_HEADERS: [&[u8]; 3] = [b"\x1b]4;", b"\x1b]10;", b"\x1b]11;"];

/// Physical terminal replies carry no server token. A timeout releases the
/// logical request but cannot retract its write to the terminal. On recovery
/// wait for those old terminators before emitting another query of this kind.
#[derive(Default)]
struct TerminalQueries {
    outstanding: u64,
    retired: u64,
    waiting: Option<Vec<u8>>,
}

impl TerminalQueries {
    fn retire(&mut self) {
        self.retired = self.outstanding;
        self.waiting = None;
    }

    fn observe_terminator(&mut self) -> bool {
        self.outstanding = self.outstanding.saturating_sub(1);
        let retired = self.retired != 0;
        self.retired = self.retired.saturating_sub(1);
        retired
    }

    fn take_ready(&mut self) -> Option<Vec<u8>> {
        if self.retired != 0 {
            return None;
        }
        let query = self.waiting.take()?;
        self.outstanding = self.outstanding.saturating_add(1);
        Some(query)
    }
}

/// Continuous host-reply parser. Lives for the whole client session.
pub struct StdinAnsiParser {
    inner: InputParser,
    partial_buffer_limit: usize,
    /// Active forwarding slot: `Some` while a forwarded query is in
    /// flight, `None` otherwise.
    pub(crate) forward_generation: Option<u64>,
    barriers: TerminalQueries,
    // Number of earlier physical DAs still due before the startup probe's DA.
    startup_barrier_after: Option<u64>,
    clipboards: TerminalQueries,
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
    recovery_partial: Option<RecoveryPartial>,
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
            forward_generation: None,
            barriers: Default::default(),
            startup_barrier_after: None,
            clipboards: Default::default(),
            active_forward: None,
            active_clipboard_forward: None,
            partial_osc: Vec::new(),
            partial_csi: Vec::new(),
            partial_paste: Vec::new(),
            nested_frame_extractor: nested_session::NestedFrameExtractor::new(),
            in_bracketed_paste: false,
            recovery_partial: None,
            startup_kitty_probe: StartupKittyProbe::NotSent,
            partial_apc: Vec::new(),
        }
    }

    pub(crate) fn take_for_next_attachment(&mut self) -> Self {
        // Leave source timers with an empty parser, even when both local connections
        // use the same token and neither has a remote recovery generation.
        let mut next = std::mem::replace(self, Self::new());
        next.barriers.retire();
        next.clipboards.retire();
        next.take_active_forward();
        next.take_active_clipboard_forward();
        next.forward_generation = None;
        next.startup_barrier_after = None;
        next.startup_kitty_probe = StartupKittyProbe::NotSent;
        next.reset_for_recovery();
        next
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

    pub(crate) fn begin_forward_generation(&mut self, origin: Option<u64>) {
        if self.forward_generation != origin {
            self.startup_barrier_after = None;
            self.barriers.retire();
            self.clipboards.retire();
            self.take_active_forward();
            self.take_active_clipboard_forward();
            self.forward_generation = origin;
        }
    }

    pub(crate) fn startup_barrier_sent(&mut self, origin: Option<u64>) {
        self.begin_forward_generation(origin);
        self.startup_barrier_after = Some(self.barriers.outstanding);
        // A new probe can already be on the terminal behind the source's query.
        // Drain its batch too before writing the waiting pane query.
        if self.barriers.retired != 0 {
            self.barriers.retired += 1;
        }
        self.barriers.outstanding += 1;
    }

    fn observe_barrier(&mut self) -> (bool, bool) {
        let startup = self.startup_barrier_after == Some(0);
        self.startup_barrier_after = self.startup_barrier_after.and_then(|n| n.checked_sub(1));
        (self.barriers.observe_terminator(), startup)
    }

    pub(crate) fn queue_terminal_query(&mut self, kind: ForwardKind, mut bytes: Vec<u8>) {
        bytes.extend_from_slice(kind.barrier_suffix());
        match kind {
            ForwardKind::Barrier => self.barriers.waiting = Some(bytes),
            ForwardKind::Clipboard => self.clipboards.waiting = Some(bytes),
        }
    }

    pub(crate) fn has_ready_terminal_query(&self) -> bool {
        [&self.barriers, &self.clipboards]
            .iter()
            .any(|queries| queries.retired == 0 && queries.waiting.is_some())
    }

    pub(crate) fn ready_terminal_queries(&mut self) -> impl Iterator<Item = Vec<u8>> + use<> {
        [self.barriers.take_ready(), self.clipboards.take_ready()]
            .into_iter()
            .flatten()
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
                self.barriers.waiting = None;
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
        self.barriers.waiting = None;
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
                self.clipboards.waiting = None;
                let slot = self.active_clipboard_forward.take().unwrap();
                Some((slot.token, Vec::new()))
            },
            _ => None,
        }
    }

    pub fn take_active_clipboard_forward(&mut self) -> Option<(u32, Vec<u8>)> {
        self.clipboards.waiting = None;
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
        let mut out = ParseOutput::default();
        let mut keyboard = Vec::new();
        let bytes = self.resume_partial(bytes, &mut out.replies, &mut keyboard);
        let bytes = if keyboard.is_empty() {
            bytes
        } else {
            keyboard.extend_from_slice(&bytes);
            Cow::Owned(keyboard)
        };
        let (bytes, nested_frames) = self.nested_frame_extractor.extract(&bytes);
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
                    }
                    if payload.starts_with(b"52;") && self.clipboards.observe_terminator() {
                        continue;
                    }
                    if payload.starts_with(b"52;")
                        && self.clipboards.waiting.is_none()
                        && let Some(slot) = self.active_clipboard_forward.take()
                    {
                        let mut bytes = b"\x1b]".to_vec();
                        bytes.extend_from_slice(&payload);
                        bytes.extend_from_slice(b"\x1b\\");
                        out.completed_clipboard_forward = Some((slot.token, bytes));
                        continue;
                    }
                    if let Some(reply) = HostReply::from_osc_payload(&payload) {
                        out.replies.push(reply);
                    }
                    if self.barriers.retired != 0 || self.barriers.waiting.is_some() {
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
                            let (retired, startup) = self.observe_barrier();
                            if !startup && (retired || self.barriers.waiting.is_some()) {
                                continue;
                            }
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
                            if !startup && let Some(slot) = self.active_forward.take() {
                                out.completed_forward = Some((slot.token, slot.reply_bytes));
                            }
                            // Primary-DA is NOT double-dispatched — it has
                            // no cached-state counterpart.
                        },
                        _ => {
                            if let Some(reply) = HostReply::from_csi_report(&raw) {
                                out.replies.push(reply);
                            }
                            if self.barriers.retired != 0 || self.barriers.waiting.is_some() {
                                continue;
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
                        self.clipboards.waiting = None;
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
        out.has_partial_state = self.pending_partial() != PendingPartial::None;
        out
    }

    pub fn pending_partial(&self) -> PendingPartial {
        if let Some(RecoveryPartial::Prefix {
            bytes,
            fresh_at,
            boundary,
        }) = &self.recovery_partial
        {
            // An idle tick cannot distinguish a pause inside a physical reply
            // from a key. Keep the usual keyboard deadline for unrelated input.
            let retain = match boundary {
                InputBoundary::Idle => self.barriers.retired != 0 || self.clipboards.retired != 0,
                InputBoundary::Recovery => self.is_query_prefix(bytes),
                InputBoundary::FirstRepaint => true,
            };
            if retain {
                return PendingPartial::None;
            }
            return match &bytes[*fresh_at..] {
                [] => PendingPartial::None,
                [0x1b] => PendingPartial::LoneEsc,
                _ => PendingPartial::ReplyInProgress,
            };
        }
        if self.recovery_partial.is_some() {
            return PendingPartial::None;
        }
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
        match self.pending_partial() {
            PendingPartial::LoneEsc | PendingPartial::BareIntroducer => self.finalize_force(),
            PendingPartial::None | PendingPartial::ReplyInProgress => Vec::new(),
        }
    }

    pub fn finalize_force(&mut self) -> Vec<u8> {
        if self.in_bracketed_paste || self.pending_partial() == PendingPartial::None {
            return Vec::new();
        }
        if let Some(RecoveryPartial::Prefix {
            bytes, fresh_at, ..
        }) = &mut self.recovery_partial
        {
            let keyboard = bytes[*fresh_at..].to_vec();
            *fresh_at = bytes.len();
            return keyboard;
        }
        let pending = self.take_partial_bytes();
        if let Some(kind) = self.partial_osc_kind(&pending, InputBoundary::Idle) {
            // An idle tick does not retract the physical query. Preserve its
            // reply for this owner, or just its framing if recovery intervenes.
            self.inner = InputParser::new();
            self.recovery_partial = Some(RecoveryPartial::Osc(PartialOsc::new(pending, kind)));
            return Vec::new();
        }
        if !pending.is_empty() && self.is_recovery_prefix(&pending, InputBoundary::Idle) {
            // An ambiguous Escape still reaches the keyboard on its usual timer.
            // Keep enough framing to recognize the physical reply if it follows.
            self.inner = InputParser::new();
            let retired = if b"\x1b]52;".starts_with(&pending) {
                self.clipboards.retired
            } else {
                self.barriers.retired
            };
            let keyboard = if pending.len() > 2 && retired != 0 {
                Vec::new()
            } else {
                pending.clone()
            };
            self.recovery_partial = Some(RecoveryPartial::Prefix {
                fresh_at: keyboard.len(),
                bytes: pending,
                boundary: InputBoundary::Idle,
            });
            return keyboard;
        }
        pending
    }

    fn take_partial_bytes(&mut self) -> Vec<u8> {
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
        self.reset_input(InputBoundary::Recovery);
    }

    /// Local host observations started while this generation was repainting
    /// still describe the same terminal when its keyboard becomes eligible.
    pub(crate) fn input_stamp_changed(
        &mut self,
        before: Option<(u64, bool)>,
        after: Option<(u64, bool)>,
    ) {
        let same_repaint =
            matches!((before, after), (Some((old, false)), Some((new, true))) if old == new);
        self.reset_input(if same_repaint {
            InputBoundary::FirstRepaint
        } else {
            InputBoundary::Recovery
        });
    }

    fn reset_input(&mut self, boundary: InputBoundary) {
        let in_paste = self.in_bracketed_paste;
        self.in_bracketed_paste = false;
        let pending = self.take_partial_bytes();
        match &mut self.recovery_partial {
            Some(RecoveryPartial::Osc(osc)) if osc.is_clipboard() => {
                if boundary == InputBoundary::Recovery || self.clipboards.retired != 0 {
                    osc.kind = OscKind::RetiredClipboard;
                }
                osc.mark_boundary();
            },
            Some(RecoveryPartial::Osc(osc)) if boundary == InputBoundary::FirstRepaint => {
                osc.mark_boundary()
            },
            Some(RecoveryPartial::Osc(_)) => self.recovery_partial = None,
            Some(RecoveryPartial::Prefix {
                bytes,
                fresh_at,
                boundary: previous,
            }) => {
                *fresh_at = bytes.len();
                if *previous != InputBoundary::Recovery {
                    *previous = boundary;
                }
            },
            _ => {},
        }
        self.inner = InputParser::new();
        if self.recovery_partial.is_none() && (in_paste || !pending.is_empty()) {
            self.recovery_partial = if in_paste {
                let matched = (1..PASTE_END_MARKER.len())
                    .rev()
                    .find(|&len| pending.ends_with(&PASTE_END_MARKER[..len]))
                    .unwrap_or(0);
                Some(RecoveryPartial::Paste(matched))
            } else if let Some(kind) = self.partial_osc_kind(&pending, boundary) {
                Some(RecoveryPartial::Osc(PartialOsc::new(pending, kind)))
            } else if self.is_recovery_prefix(&pending, boundary) {
                Some(RecoveryPartial::Prefix {
                    fresh_at: pending.len(),
                    bytes: pending,
                    boundary,
                })
            } else {
                None
            };
        }
    }

    fn partial_osc_kind(&self, bytes: &[u8], boundary: InputBoundary) -> Option<OscKind> {
        if self.clipboards.outstanding != 0 && bytes.starts_with(b"\x1b]52;") {
            return Some(
                if boundary == InputBoundary::Recovery || self.clipboards.retired != 0 {
                    OscKind::RetiredClipboard
                } else {
                    OscKind::Clipboard
                },
            );
        }
        if !self.preserves_host_observations(boundary) {
            return None;
        }
        let header = HOST_COLOR_HEADERS.iter().find(|h| bytes.starts_with(h))?;
        let prefix = ColorPrefix::from_report(bytes, header);
        Some(OscKind::HostColor {
            prefix,
            complete_at_boundary: prefix.complete(),
        })
    }

    fn preserves_host_observations(&self, boundary: InputBoundary) -> bool {
        boundary == InputBoundary::FirstRepaint
            || (boundary == InputBoundary::Idle && self.barriers.retired != 0)
    }

    fn is_query_prefix(&self, bytes: &[u8]) -> bool {
        (self.clipboards.outstanding != 0 && b"\x1b]52;".starts_with(bytes))
            || (self.barriers.outstanding != 0
                && (b"\x1b[".starts_with(bytes)
                    || matches!(csi_status(bytes), SeqStatus::NeedMore)))
    }

    fn is_recovery_prefix(&self, bytes: &[u8], boundary: InputBoundary) -> bool {
        self.is_query_prefix(bytes)
            || (boundary != InputBoundary::Idle && PASTE_START_MARKER.starts_with(bytes))
            || (self.preserves_host_observations(boundary)
                && (HOST_COLOR_HEADERS.iter().any(|h| h.starts_with(bytes))
                    || matches!(csi_status(bytes), SeqStatus::NeedMore)))
    }

    fn resume_partial<'a>(
        &mut self,
        bytes: &'a [u8],
        replies: &mut Vec<HostReply>,
        keyboard: &mut Vec<u8>,
    ) -> Cow<'a, [u8]> {
        for (i, &byte) in bytes.iter().enumerate() {
            match self.recovery_partial.take() {
                None => return Cow::Borrowed(&bytes[i..]),
                Some(RecoveryPartial::Prefix {
                    bytes: mut prefix,
                    fresh_at,
                    boundary,
                }) => {
                    prefix.push(byte);
                    self.recovery_partial =
                        if boundary != InputBoundary::Idle && prefix == PASTE_START_MARKER {
                            Some(RecoveryPartial::Paste(0))
                        } else if let Some(kind) = self.partial_osc_kind(&prefix, boundary) {
                            Some(RecoveryPartial::Osc(PartialOsc::new(prefix, kind)))
                        } else if boundary != InputBoundary::Recovery
                            && matches!(csi_status(&prefix), SeqStatus::Complete(_))
                        {
                            prefix.extend_from_slice(&bytes[i + 1..]);
                            return Cow::Owned(prefix);
                        } else if self.barriers.outstanding != 0
                            && byte == b'c'
                            && matches!(csi_status(&prefix), SeqStatus::Complete(_))
                        {
                            self.observe_barrier();
                            None
                        } else if matches!(csi_status(&prefix), SeqStatus::Complete(_))
                            && let Some(report) = HostReply::from_csi_report(&prefix)
                        {
                            replies.push(report);
                            None
                        } else if self.is_recovery_prefix(&prefix, boundary) {
                            Some(RecoveryPartial::Prefix {
                                bytes: prefix,
                                fresh_at,
                                boundary,
                            })
                        } else {
                            let mut keyboard = prefix.split_off(fresh_at);
                            keyboard.extend_from_slice(&bytes[i + 1..]);
                            return Cow::Owned(keyboard);
                        };
                },
                Some(RecoveryPartial::Osc(mut osc)) => {
                    let escaped = osc.bytes.ends_with(b"\x1b");
                    if (!escaped && byte == 7) || (escaped && byte == b'\\') {
                        if osc.kind == OscKind::RetiredClipboard {
                            self.clipboards.observe_terminator();
                        } else if !matches!(osc.kind, OscKind::InterruptedColor { complete: false })
                        {
                            // Classify the complete live OSC through the ordinary decoder.
                            osc.bytes.extend_from_slice(&bytes[i..]);
                            return Cow::Owned(osc.bytes);
                        }
                    } else if !escaped
                        && byte != 0x1b
                        && osc.bytes.len() < self.partial_buffer_limit
                        && osc.consume_color_byte(byte, keyboard)
                    {
                        self.recovery_partial = Some(RecoveryPartial::Osc(osc));
                    } else if escaped
                        || !osc.accepts(byte)
                        || osc.bytes.len() >= self.partial_buffer_limit
                    {
                        let mut keyboard = osc.keyboard_suffix();
                        keyboard.extend_from_slice(&bytes[i..]);
                        return Cow::Owned(keyboard);
                    } else {
                        if osc.kind != OscKind::RetiredClipboard || byte == 0x1b {
                            osc.bytes.push(byte);
                        }
                        self.recovery_partial = Some(RecoveryPartial::Osc(osc));
                    }
                },
                Some(RecoveryPartial::Paste(matched)) => {
                    let matched = if byte == PASTE_END_MARKER[matched] {
                        matched + 1
                    } else {
                        usize::from(byte == PASTE_END_MARKER[0])
                    };
                    if matched != PASTE_END_MARKER.len() {
                        self.recovery_partial = Some(RecoveryPartial::Paste(matched));
                    }
                },
            }
        }
        Cow::Borrowed(&[])
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

//! Classifying host-terminal replies.
//!
//! Purely functional: each constructor takes the raw bytes of one reply and
//! decides whether it is something the client knows how to consume. Nothing
//! here touches parser state.

use crate::stdin_ansi_parser::SyncOutput;
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use yinz_types::pane_size::SizeInPixels;
use {yinz_ipc::ipc::PixelDimensions, yinz_types::data::HostTerminalThemeMode};

/// A classified host-terminal reply received on stdin.
///
/// The variants track the reply types Yinz consumes for its own
/// synchronous render hot-path (pixel dims, bg/fg, palette registers,
/// sync-output support). Accumulated forwarded-query byte streams take
/// a separate path: they ride `ParseOutput::completed_forward` through
/// a dedicated input-instruction channel so they don't get co-mingled
/// with semantically-typed state updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostReply {
    PixelDimensions(PixelDimensions),
    BackgroundColor(String),
    ForegroundColor(String),
    ColorRegisters(Vec<(usize, String)>),
    SynchronizedOutput(Option<SyncOutput>),
    /// DSR 997 reply / unsolicited notification reporting the host
    /// terminal's color-palette theme mode (CSI 2031).
    HostTerminalThemeChanged(HostTerminalThemeMode),
    KittyGraphicsSupport(bool),
    SixelSupport(bool),
}

/// Retained alias for the pre-refactor type name used by other modules in
/// the client. New code should prefer `HostReply`; the alias keeps the
/// existing `InputInstruction::AnsiStdinInstructions(Vec<...>)` plumbing
/// stable during the migration.
pub type AnsiStdinInstruction = HostReply;

impl HostReply {
    /// Classify an OSC payload (the bytes between the `ESC ]` prefix and
    /// the ST/BEL terminator) into a known `HostReply`, if possible.
    pub fn from_osc_payload(payload: &[u8]) -> Option<HostReply> {
        lazy_static! {
            // OSC 10 (foreground) / OSC 11 (background) answer form:
            //   OSC 10 ; <color> ST        e.g. "10;rgb:ffff/ffff/ffff"
            //   OSC 11 ; <color> ST
            static ref FG_RE: Regex = Regex::new(r"^10;(.*)$").unwrap();
            static ref BG_RE: Regex = Regex::new(r"^11;(.*)$").unwrap();
            // OSC 4 ; N ; <color> — palette-register answer.
            static ref COLOR_REGISTER_RE: Regex = Regex::new(r"^4;(\d+);(.*)$").unwrap();
        }
        let s = std::str::from_utf8(payload).ok()?;
        if let Some(caps) = BG_RE.captures(s) {
            return Some(HostReply::BackgroundColor(caps[1].to_string()));
        }
        if let Some(caps) = FG_RE.captures(s) {
            return Some(HostReply::ForegroundColor(caps[1].to_string()));
        }
        if let Some(caps) = COLOR_REGISTER_RE.captures(s) {
            let index: usize = caps[1].parse().ok()?;
            let color = caps[2].to_string();
            return Some(HostReply::ColorRegisters(vec![(index, color)]));
        }
        None
    }

    /// Classify a CSI-based report (the full raw sequence including the
    /// leading `ESC [`) into a `HostReply`, if possible.
    ///
    /// Recognised final bytes: `t` (pixel-dims reply, `CSI 4/6 ; H ; W t`),
    /// `y` (DECRPM reply to `CSI ?2026$p` — sync-output support
    /// advertisement).
    pub fn from_csi_report(raw: &[u8]) -> Option<HostReply> {
        let s = std::str::from_utf8(raw).ok()?;
        lazy_static! {
            // <ESC>[4;H;Wt or <ESC>[6;H;Wt
            static ref PIX_RE: Regex = Regex::new(r"^\u{1b}\[(\d+);(\d+);(\d+)t$").unwrap();
            // <ESC>[?2026;Ny — DECRPM reply for sync-output (VT mode 2026)
            static ref SYNC_RE: Regex = Regex::new(r"^\u{1b}\[\?2026;([0-4])\$y$").unwrap();
            // <ESC>[?997;1n (dark) / <ESC>[?997;2n (light) — DSR 997 reply
            // to CSI ?996n, or unsolicited host-theme notification when
            // CSI ?2031h is enabled.
            static ref THEME_RE: Regex = Regex::new(r"^\u{1b}\[\?997;([12])n$").unwrap();
        }
        if let Some(caps) = PIX_RE.captures(s) {
            let which: usize = caps[1].parse().ok()?;
            let first: usize = caps[2].parse().ok()?;
            let second: usize = caps[3].parse().ok()?;
            return match which {
                4 => Some(HostReply::PixelDimensions(PixelDimensions {
                    text_area_grid_size: None,
                    character_cell_size: None,
                    text_area_size: Some(SizeInPixels {
                        height: first,
                        width: second,
                    }),
                })),
                6 => Some(HostReply::PixelDimensions(PixelDimensions {
                    text_area_grid_size: None,
                    character_cell_size: Some(SizeInPixels {
                        height: first,
                        width: second,
                    }),
                    text_area_size: None,
                })),
                _ => None,
            };
        }
        if let Some(caps) = SYNC_RE.captures(s) {
            let code: usize = caps[1].parse().ok()?;
            return match code {
                1..=3 => Some(HostReply::SynchronizedOutput(Some(SyncOutput::Csi))),
                _ => Some(HostReply::SynchronizedOutput(None)),
            };
        }
        if let Some(caps) = THEME_RE.captures(s) {
            let mode = match &caps[1] {
                "1" => HostTerminalThemeMode::Dark,
                "2" => HostTerminalThemeMode::Light,
                _ => return None,
            };
            return Some(HostReply::HostTerminalThemeChanged(mode));
        }
        None
    }

    /// Classify a Primary Device Attributes reply (`CSI ? Ps ; Ps ... c`)
    /// into a Sixel-capability advertisement. Attribute `4` in the reply
    /// means the host terminal supports Sixel graphics.
    ///
    /// Replies that are not a primary-DA form (eg. secondary-DA `CSI > ... c`)
    /// yield `None` so they do not clobber the host capability state.
    pub fn sixel_support_from_primary_da(raw: &[u8]) -> Option<HostReply> {
        lazy_static! {
            static ref PRIMARY_DA_RE: Regex = Regex::new(r"^\u{1b}\[\?([0-9;]*)c$").unwrap();
        }
        let s = std::str::from_utf8(raw).ok()?;
        let caps = PRIMARY_DA_RE.captures(s)?;
        let supports_sixel = caps[1].split(';').any(|attribute| attribute == "4");
        Some(HostReply::SixelSupport(supports_sixel))
    }
}

//! Desktop notifications between an app in a pane and the host terminal.
//!
//! An app asks for one with OSC 9, 99 or 777, which the grid records as a
//! [`PendingNotification`]. A host that speaks OSC 99 is sent the escape itself, its identifier
//! namespaced to the pane ([`namespace_notification_id`]), and its answer is routed back with the
//! app's own identifier restored ([`denormalize_notification_response`]). Every other host is
//! sent the title and body in the protocol it speaks ([`NotificationProtocol`]).

use std::collections::BTreeMap;

use yinz_types::input::options::HostNotificationProtocol;
use yinz_types::internal_ids::TerminalId;
use yinz_types::text::clean_string_from_control_and_linebreak;

/// A desktop notification an application asked for, in the protocol it used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingNotification {
    /// kitty's protocol. `payload` is the semicolon-joined params after `99`, `terminator` is
    /// `\x07` or `\x1b\\`, `wants_report` whether the app asked for an activation report, and
    /// `display` the assembled title and body once the notification is complete (`None` for an
    /// unfinished chunk or a control request that shows nothing).
    Osc99 {
        payload: String,
        terminator: String,
        wants_report: bool,
        display: Option<(String, String)>,
    },
    /// The legacy iTerm2 protocol: a body and nothing else.
    Osc9 { body: String },
    /// The urxvt/rxvt protocol: `notify;title;body`.
    Osc777 { title: String, body: String },
}

impl PendingNotification {
    pub fn title_and_body(&self) -> (String, String) {
        match self {
            PendingNotification::Osc99 { display, .. } => display.clone().unwrap_or_default(),
            PendingNotification::Osc9 { body } => (String::new(), body.clone()),
            PendingNotification::Osc777 { title, body } => (title.clone(), body.clone()),
        }
    }
}

/// The `p=` key of an OSC 99 escape: what the payload is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99PayloadType {
    Title,
    Body,
    Close,
    Query,
    Alive,
    Other,
}

impl Osc99PayloadType {
    pub fn from_metadata_value(value: &str) -> Self {
        match value {
            "title" => Osc99PayloadType::Title,
            "body" => Osc99PayloadType::Body,
            "close" => Osc99PayloadType::Close,
            "?" => Osc99PayloadType::Query,
            "alive" => Osc99PayloadType::Alive,
            _ => Osc99PayloadType::Other,
        }
    }
    /// A request the app expects an answer to, rather than something to show.
    pub fn is_control_request(&self) -> bool {
        matches!(
            self,
            Osc99PayloadType::Close | Osc99PayloadType::Query | Osc99PayloadType::Alive
        )
    }
}

// The three parsing helpers below are public only for yinz-terminal's grid: its OSC 99
// dispatch and `NotificationTracker` stay there because the grid tests read the tracker's private
// fields. They are hidden from the docs: they are not part of the protocol.

/// Splits an OSC 99 payload into its metadata (before the first `;`) and the rest.
#[doc(hidden)]
pub fn split_osc99_payload(payload: &str) -> (&str, &str) {
    match payload.find(';') {
        Some(idx) => (
            payload.get(..idx).unwrap_or_default(),
            payload.get(idx + 1..).unwrap_or_default(),
        ),
        None => (payload, ""),
    }
}

/// The `key=value` pairs of an OSC 99 metadata section.
#[doc(hidden)]
pub fn parse_osc99_metadata(metadata: &str) -> BTreeMap<&str, &str> {
    metadata
        .split(':')
        .filter_map(|kv| kv.split_once('='))
        .collect()
}

/// Whether an OSC 99 `a=` value asks the terminal for an activation report.
#[doc(hidden)]
pub fn action_wants_report(action_value: &str) -> bool {
    action_value.split(',').any(|value| value == "report")
}

/// Rewrites OSC 99 metadata for multiplexer forwarding:
///
/// 1. Namespaces the `i=` value as `i=p<pane_id>[r].<original_id>` so responses can be routed
///    back to the originating pane. The `r` flag records that the app asked for `a=report`
///    (`wants_report`, remembered per id by the pane's `NotificationTracker`), so the response
///    is handed back to it; without the flag the response is only used for focus routing.
///    If no `i=` key is present, one is added with an empty original id.
///
/// 2. Ensures `a=...,report` is present on notifications so the host terminal sends the
///    activation response back to Yinz (needed for pane focus routing). An escape without an
///    `a=` gets the protocol's default `a=focus,report`; control requests (`p=?`, `close`,
///    `alive`) are left without one, as they are not something the host reports on.
pub fn namespace_notification_id(
    metadata: &str,
    pane_id: TerminalId,
    wants_report: bool,
) -> String {
    let flags = if wants_report { "r" } else { "" };
    let mut found_id = false;
    let mut found_action = false;
    let mut payload_type = Osc99PayloadType::Title;
    let mut parts: Vec<String> = Vec::new();
    for kv in metadata.split(':') {
        if kv.is_empty() {
            continue;
        }
        if let Some(id_value) = kv.strip_prefix("i=") {
            found_id = true;
            parts.push(format!("i=p{}{}.{}", pane_id, flags, id_value));
        } else if let Some(action_value) = kv.strip_prefix("a=") {
            found_action = true;
            if action_wants_report(action_value) {
                parts.push(kv.to_owned());
            } else {
                parts.push(format!("a={},report", action_value));
            }
        } else {
            if let Some(payload_value) = kv.strip_prefix("p=") {
                payload_type = Osc99PayloadType::from_metadata_value(payload_value);
            }
            parts.push(kv.to_owned());
        }
    }
    if !found_id {
        parts.insert(0, format!("i=p{}{}.", pane_id, flags));
    }
    if !found_action && !payload_type.is_control_request() {
        parts.push("a=focus,report".to_owned());
    }
    parts.join(":")
}

/// What the screen does with a host terminal's answer to a forwarded OSC 99 notification.
pub struct NotificationResponse {
    /// The terminal pane that sent the notification.
    pub terminal_id: TerminalId,
    /// Whether the answer is written to that pane's pty: control requests (`p=?`, `close`,
    /// `alive`) are always answered, an activation report only if the app asked for one.
    pub forward_to_pane: bool,
    /// Whether the pane is focused: an activation (the user clicked the notification) focuses
    /// it, an answer to a control request does not.
    pub focus_pane: bool,
    /// The complete OSC 99 sequence with the app's own identifier restored.
    pub bytes: Vec<u8>,
}

/// Denormalizes a namespaced OSC 99 response.
///
/// Input bytes (the termwiz OperatingSystemCommand payload after stripping "99;"):
/// e.g. b"i=p42.mynotif" or b"i=p42r.mynotif:p=close;some_data"
///
/// Parses the namespaced `i=p<N>[r].<original_id>` format (`r`: the app asked for
/// `a=report`), restores the original identifier (`i=0`, the protocol's default, when the app
/// never sent one) and, for a `p=alive` answer, keeps only the identifiers that belong to the
/// asking pane, un-namespaced.
pub fn denormalize_notification_response(payload: &[u8]) -> Option<NotificationResponse> {
    let payload_str = str::from_utf8(payload).ok()?;

    // Split into metadata and response payload on first ';'
    let (metadata, response_payload) = match payload_str.find(';') {
        Some(idx) => (
            payload_str.get(..idx).unwrap_or_default(),
            payload_str.get(idx..).unwrap_or_default(),
        ),
        None => (payload_str, ""),
    };

    // Find the i= key in colon-separated metadata
    let mut terminal_id = None;
    let mut app_wants_report = false;
    let mut payload_type = Osc99PayloadType::Other;
    let mut restored_parts = Vec::new();

    for kv in metadata.split(':') {
        if let Some(namespaced_value) = kv.strip_prefix("i=p")
            && let Some(dot_pos) = namespaced_value.find('.')
        {
            let flags_part = namespaced_value.get(..dot_pos).unwrap_or_default();
            let original_id = namespaced_value.get(dot_pos + 1..).unwrap_or_default();
            let digits = flags_part
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(flags_part.len());
            let pane_id_str = flags_part.get(..digits).unwrap_or_default();
            let flag_chars = flags_part.get(digits..).unwrap_or_default();
            if let Ok(pid) = pane_id_str.parse::<u32>() {
                terminal_id = Some(pid);
                app_wants_report = flag_chars.contains('r');
                if original_id.is_empty() {
                    restored_parts.push("i=0".to_owned());
                } else {
                    restored_parts.push(format!("i={}", original_id));
                }
                continue;
            }
        }
        if let Some(payload_value) = kv.strip_prefix("p=") {
            payload_type = Osc99PayloadType::from_metadata_value(payload_value);
        }
        restored_parts.push(kv.to_string());
    }

    let terminal_id = terminal_id?;
    let response_payload = if payload_type == Osc99PayloadType::Alive {
        let prefix_with_report = format!("p{}r.", terminal_id);
        let prefix = format!("p{}.", terminal_id);
        let alive_ids: Vec<&str> = response_payload
            .trim_start_matches(';')
            .split(',')
            .filter_map(|id| {
                id.strip_prefix(&prefix_with_report)
                    .or_else(|| id.strip_prefix(&prefix))
            })
            .map(|id| if id.is_empty() { "0" } else { id })
            .collect();
        format!(";{}", alive_ids.join(","))
    } else {
        response_payload.to_owned()
    };
    let restored_metadata = restored_parts.join(":");
    let full_response = format!("\x1b]99;{}{}\x1b\\", restored_metadata, response_payload);
    Some(NotificationResponse {
        terminal_id: TerminalId::new(terminal_id),
        forward_to_pane: app_wants_report || payload_type.is_control_request(),
        focus_pane: !payload_type.is_control_request(),
        bytes: full_response.into_bytes(),
    })
}

/// The protocol notifications are sent to one host terminal with, once `auto` is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotificationProtocol {
    Osc99,
    #[default]
    Osc9,
    Bell,
    Off,
}

impl NotificationProtocol {
    /// kitty speaks OSC 99; everything else is assumed to understand the legacy OSC 9.
    pub fn from_host_terminal_env(host_terminal_env: &BTreeMap<String, String>) -> Self {
        let is_kitty = host_terminal_env
            .get("TERM")
            .map(|term| term == "xterm-kitty")
            .unwrap_or(false)
            || host_terminal_env.contains_key("KITTY_WINDOW_ID");
        if is_kitty {
            NotificationProtocol::Osc99
        } else {
            NotificationProtocol::Osc9
        }
    }
    pub fn resolve(
        configured: HostNotificationProtocol,
        host_terminal_env: &BTreeMap<String, String>,
    ) -> Self {
        match configured {
            HostNotificationProtocol::Auto => Self::from_host_terminal_env(host_terminal_env),
            HostNotificationProtocol::Osc9 => NotificationProtocol::Osc9,
            HostNotificationProtocol::Osc99 => NotificationProtocol::Osc99,
            HostNotificationProtocol::Bell => NotificationProtocol::Bell,
            HostNotificationProtocol::Off => NotificationProtocol::Off,
        }
    }
    /// The bytes to send the host for a notification with this title and body, or `None` when
    /// the host is not to be told at all.
    pub fn render(&self, title: &str, body: &str) -> Option<String> {
        let payload = if title.is_empty() {
            body.to_owned()
        } else if body.is_empty() {
            title.to_owned()
        } else {
            format!("{}: {}", title, body)
        };
        // OSC 99 can carry base64-decoded text, including terminal control sequences.
        let payload = clean_string_from_control_and_linebreak(&payload);
        match self {
            NotificationProtocol::Osc99 => Some(format!("\u{1b}]99;;{}\u{7}", payload)),
            NotificationProtocol::Osc9 => Some(format!("\u{1b}]9;{}\u{7}", payload)),
            NotificationProtocol::Bell => Some("\u{7}".to_owned()),
            NotificationProtocol::Off => None,
        }
    }
}

#[cfg(test)]
#[path = "./unit/notifications_tests.rs"]
mod notifications_tests;

//! Which desktop-notification protocol each client's host terminal speaks, and how a
//! notification is rendered in it.

use std::collections::BTreeMap;

use zellij_types::input::options::HostNotificationProtocol;
use zellij_types::text::clean_string_from_control_and_linebreak;

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

//! The `web { }` config block: the session's mobile web interface.
//!
//! ```kdl
//! web {
//!     enabled true
//!     // bind "127.0.0.1"
//!     // port 7419
//!     // plugin "web"
//!     // allowed_hosts "box.tail1234.ts.net"
//!     // allowed_origins "https://box.tail1234.ts.net:7443"
//!     // tailscale_users "me@github"
//!     // session_days 30
//!     // https_port_hint 7443
//! }
//! ```
//!
//! A field of `Config`, not of `Options`, so it never crosses the client-server protobuf. It
//! takes effect when a session starts; a password file is a serving condition, not a start
//! condition.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde::{Deserialize, Serialize};

pub const DEFAULT_WEB_BIND: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
pub const DEFAULT_WEB_PORT: u16 = 7419;
pub const DEFAULT_WEB_PLUGIN: &str = "web";
pub const DEFAULT_WEB_SESSION_DAYS: u32 = 30;
pub const WEB_SESSION_DAYS_RANGE: std::ops::RangeInclusive<u32> = 1..=90;
pub const DEFAULT_WEB_HTTPS_PORT_HINT: u16 = 7443;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebConfig {
    /// Default false.
    pub enabled: Option<bool>,
    /// An IP address; default `127.0.0.1`.
    pub bind: Option<String>,
    /// Default 7419; 0 binds an ephemeral port.
    pub port: Option<u16>,
    /// Required for a non-loopback `bind`.
    pub allow_non_loopback: Option<bool>,
    /// Alias or URL of the route plugin; default alias `web`.
    pub plugin: Option<String>,
    /// Extra exact hosts, added to loopback and `*.ts.net`.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Extra exact origins, `scheme://host:port`.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Tailscale logins allowed in addition to the password; empty turns the check off.
    #[serde(default)]
    pub tailscale_users: Vec<String>,
    /// Absolute session lifetime, 1..=90 days; default 30.
    pub session_days: Option<u32>,
    /// The `tailscale serve --https` port printed in help text; default 7443.
    pub https_port_hint: Option<u16>,
}

impl WebConfig {
    /// Field by field, `other`'s `Some` (or non-empty list) wins.
    pub fn merge(self, other: Self) -> Self {
        fn list(mine: Vec<String>, theirs: Vec<String>) -> Vec<String> {
            if theirs.is_empty() {
                mine
            } else {
                theirs
            }
        }
        WebConfig {
            enabled: other.enabled.or(self.enabled),
            bind: other.bind.or(self.bind),
            port: other.port.or(self.port),
            allow_non_loopback: other.allow_non_loopback.or(self.allow_non_loopback),
            plugin: other.plugin.or(self.plugin),
            allowed_hosts: list(self.allowed_hosts, other.allowed_hosts),
            allowed_origins: list(self.allowed_origins, other.allowed_origins),
            tailscale_users: list(self.tailscale_users, other.tailscale_users),
            session_days: other.session_days.or(self.session_days),
            https_port_hint: other.https_port_hint.or(self.https_port_hint),
        }
    }

    /// Whether the config asks for the server; `bind_addr` must also succeed for it to start.
    pub fn enabled(&self) -> bool {
        self.enabled == Some(true)
    }

    /// The listen address. A non-loopback address requires `allow_non_loopback true`.
    pub fn bind_addr(&self) -> Result<SocketAddr, String> {
        let ip = match self.bind.as_deref().map(str::trim) {
            None => DEFAULT_WEB_BIND,
            Some(text) => text
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<IpAddr>()
                .map_err(|_| format!("web bind '{text}' is not an IP address"))?,
        };
        if !ip.is_loopback() && self.allow_non_loopback != Some(true) {
            return Err(format!(
                "web bind '{ip}' is not a loopback address; add `allow_non_loopback true` to \
                 listen on it"
            ));
        }
        Ok(SocketAddr::new(ip, self.port.unwrap_or(DEFAULT_WEB_PORT)))
    }

    pub fn plugin_or_default(&self) -> String {
        self.plugin
            .as_deref()
            .map(str::trim)
            .filter(|plugin| !plugin.is_empty())
            .unwrap_or(DEFAULT_WEB_PLUGIN)
            .to_owned()
    }

    pub fn session_days_or_default(&self) -> u32 {
        self.session_days.unwrap_or(DEFAULT_WEB_SESSION_DAYS)
    }

    pub fn https_port_hint_or_default(&self) -> u16 {
        self.https_port_hint.unwrap_or(DEFAULT_WEB_HTTPS_PORT_HINT)
    }

    /// Every rule a config file is held to: the bind address and the session lifetime.
    pub fn validate(&self) -> Result<(), String> {
        self.bind_addr()?;
        if let Some(days) = self.session_days
            && !WEB_SESSION_DAYS_RANGE.contains(&days)
        {
            return Err(format!(
                "web session_days {days} is outside {}..={}",
                WEB_SESSION_DAYS_RANGE.start(),
                WEB_SESSION_DAYS_RANGE.end()
            ));
        }
        Ok(())
    }
}

#[cfg(unix)]
pub use unix_only::*;

#[cfg(unix)]
mod unix_only {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::{fs, io};

    pub fn set_permissions(path: &Path, mode: u32) -> io::Result<()> {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(mode);
        fs::set_permissions(path, permissions)
    }
}

#[cfg(unix)]
#[path = "secure_dir.rs"]
mod secure_dir;

#[cfg(not(unix))]
pub fn set_permissions(_path: &std::path::Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

use crate::errors::prelude::*;
use yinz_types::internal_ids::SessionName;

/// Create private directories beneath trusted ancestors, refusing symlinks at `path`.
pub fn ensure_secure_dir(path: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    return secure_dir::ensure(path)
        .with_context(|| format!("failed to secure {}", path.display()));

    #[cfg(not(unix))]
    std::fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))
}

/// The OSC 0 sequence that sets the host terminal's title for a focused pane.
///
/// Always leads with this machine's hostname — the same name the tab bar prints in its top-left
/// corner — so a window attached to a session over SSH says which machine it is on.
pub fn make_terminal_title(pane_title: &str, session_name: Option<&SessionName>) -> String {
    let hostname = crate::host_identity::host_display_name();
    format_terminal_title(
        &hostname,
        session_name.map(|name| name.as_str()),
        pane_title,
    )
}

/// [`make_terminal_title`] with its inputs passed in: `hostname:session | pane title`, leaving
/// out whichever parts are empty.
fn format_terminal_title(hostname: &str, session_name: Option<&str>, pane_title: &str) -> String {
    let location = [hostname, session_name.unwrap_or_default()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(":");
    let title = [location.as_str(), pane_title]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    format!(
        "\u{1b}]0;{}\u{07}",
        yinz_types::text::clean_string_from_control_and_linebreak(&title)
    )
}

pub struct ServerAddress {
    pub ip: String,
    pub port: u16,
}

pub fn parse_base_url(url: &str) -> Result<ServerAddress> {
    let url = url::Url::parse(url)?;
    let ip = url
        .host_str()
        .ok_or_else(|| anyhow!("No host in URL"))?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("No port in URL"))?;

    Ok(ServerAddress { ip, port })
}

pub fn expand_path(raw: &str) -> Result<std::path::PathBuf, String> {
    shellexpand::full(raw)
        .map(|expanded| std::path::PathBuf::from(expanded.as_ref()))
        .map_err(|error| error.to_string())
}

// (this was shamelessly copied from alacritty)
//
// This returns the current terminal version as a unique number based on the
// semver version. The different versions are padded to ensure that a higher semver version will
// always report a higher version number.
pub fn version_number(mut version: &str) -> usize {
    if let Some(separator) = version.rfind('-') {
        version = &version[..separator];
    }

    let mut version_number = 0;

    let semver_versions = version.split('.');
    for (i, semver_version) in semver_versions.rev().enumerate() {
        let semver_number = semver_version.parse::<usize>().unwrap_or(0);
        version_number += usize::pow(100, i as u32) * semver_number;
    }

    version_number
}

#[cfg(test)]
#[path = "unit/shared_tests.rs"]
mod tests;

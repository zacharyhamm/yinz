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

#[cfg(not(unix))]
pub fn set_permissions(_path: &std::path::Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

use crate::envs::get_session_name;
use crate::errors::prelude::*;

pub fn make_terminal_title(pane_title: &str) -> String {
    format!(
        "\u{1b}]0;{}{}\u{07}",
        get_session_name()
            .map(|n| if pane_title.is_empty() {
                n.to_string()
            } else {
                format!("{} | ", n)
            })
            .unwrap_or_default(),
        pane_title
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

/// Expand `~` and `$VAR` in a path written by a user, in a layout or a config.
///
/// One spelling for every path property, so `~/src/project` means the same
/// thing as a pane's `cwd`, a keybind's `firecracker_worktree` or the
/// `firecracker_provider` option; the error names what could not be expanded.
pub fn expand_path(raw: &str) -> Result<std::path::PathBuf, String> {
    shellexpand::full(raw)
        .map(|expanded| std::path::PathBuf::from(expanded.as_ref()))
        .map_err(|error| error.to_string())
}

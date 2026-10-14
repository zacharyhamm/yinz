//! Shared agent configuration roots used by hook installation.
use crate::errors::prelude::*;
use std::{ffi::OsString, path::PathBuf};

fn non_empty(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub fn config_root(
    agent: &str,
    claude_home: Option<OsString>,
    codex_home: Option<OsString>,
    pi_home: Option<OsString>,
    xdg_config_home: Option<OsString>,
    home: impl FnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    Ok(match agent {
        "claude" => match non_empty(claude_home) {
            Some(root) => root,
            None => home()?.join(".claude"),
        },
        "codex" => match non_empty(codex_home) {
            Some(root) => root,
            None => home()?.join(".codex"),
        },
        // pi discovers extensions in `~/.pi/agent/extensions/` - its own convention,
        // honoured rather than redirected, because a hand-run pi loads them from there
        // with no flag at all.
        "pi" => match non_empty(pi_home) {
            Some(root) if root.as_os_str() == "~" => home()?,
            Some(root) if root.as_os_str().as_encoded_bytes().starts_with(b"~/") => home()?.join(
                root.strip_prefix("~")
                    .context("cannot expand Pi directory override")?,
            ),
            Some(root) => root,
            None => home()?.join(".pi/agent"),
        },
        // opencode reads its config from `$XDG_CONFIG_HOME/opencode`, and falls back to
        // `~/.config/opencode` where that is unset, the way opencode itself does.
        "opencode" => match non_empty(xdg_config_home) {
            Some(root) => root.join("opencode"),
            None => home()?.join(".config/opencode"),
        },
        // One root for the install and the launcher's startup verification.
        "muse" => match non_empty(xdg_config_home) {
            Some(root) => root.join("muse"),
            None => home()?.join(".config/muse"),
        },
        "agy" => home()?.join(".gemini"),
        _ => bail!("unknown agent configuration layout: {agent}"),
    })
}

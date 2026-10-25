use std::collections::BTreeMap;

/// The environment variables that identify the client's host terminal, sent to the server so
/// it can pick the desktop-notification protocol the host speaks. A whitelist: nothing else
/// from the client's environment crosses over.
pub const HOST_TERMINAL_ENV_VARS: [&str; 7] = [
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "KITTY_WINDOW_ID",
    "WEZTERM_PANE",
    "ITERM_SESSION_ID",
    "GHOSTTY_RESOURCES_DIR",
];

pub fn host_terminal_env() -> BTreeMap<String, String> {
    host_terminal_env_from(|name| std::env::var(name).ok())
}

pub fn host_terminal_env_from<F: Fn(&str) -> Option<String>>(
    lookup: F,
) -> BTreeMap<String, String> {
    HOST_TERMINAL_ENV_VARS
        .iter()
        .filter_map(|name| lookup(name).map(|value| (name.to_string(), value)))
        .collect()
}

#[cfg(test)]
#[path = "unit/host_terminal_env.rs"]
mod tests;

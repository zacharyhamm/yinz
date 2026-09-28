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
mod tests {
    use super::*;

    #[test]
    fn only_whitelisted_host_terminal_variables_are_captured() {
        let env = host_terminal_env_from(|name| match name {
            "TERM" => Some("xterm-kitty".to_owned()),
            "KITTY_WINDOW_ID" => Some("3".to_owned()),
            "SECRET_TOKEN" => Some("hunter2".to_owned()),
            _ => None,
        });
        assert_eq!(
            env,
            [
                ("TERM".to_owned(), "xterm-kitty".to_owned()),
                ("KITTY_WINDOW_ID".to_owned(), "3".to_owned())
            ]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
            "unset and non-whitelisted variables are left out"
        );
    }

    #[test]
    fn a_host_without_any_of_the_known_variables_yields_an_empty_env() {
        assert!(host_terminal_env_from(|_| None).is_empty());
    }
}

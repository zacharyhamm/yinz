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

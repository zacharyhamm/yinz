use super::*;

#[test]
fn session_identity_is_shared_without_changing_the_process_environment() {
    let inherited = std::env::var_os(SESSION_NAME_ENV_KEY);
    let state = SessionState::new("old".into());
    let reader = state.clone();
    let other_server = SessionState::new("other".into());
    std::thread::spawn(move || state.set("new".into()))
        .join()
        .unwrap();
    assert_eq!(reader.current().as_str(), "new");
    assert_eq!(other_server.current().as_str(), "other");
    assert_eq!(std::env::var_os(SESSION_NAME_ENV_KEY), inherited);
}

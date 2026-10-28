use super::plugins::max_session_name_bytes;
use std::path::Path;
use yinz_runtime::consts::YINZ_SOCK_MAX_LENGTH;

#[test]
fn session_name_limit_reserves_directory_separator_and_nul_bytes() {
    for directory in ["/tmp/yinz", "/tmp/yinz/", "/tmp/é", "/", ""] {
        let directory = Path::new(directory);
        let max_bytes = max_session_name_bytes(directory);
        let longest_path = directory.join("x".repeat(max_bytes));
        assert_eq!(longest_path.as_os_str().len(), YINZ_SOCK_MAX_LENGTH - 1);
    }
    assert_eq!(
        max_session_name_bytes(Path::new(&"x".repeat(YINZ_SOCK_MAX_LENGTH))),
        0
    );
}

#[test]
fn shared_plugin_identity_tracks_rename_without_changing_incarnation() {
    let state = yinz_runtime::envs::SessionState::new("original-session".into());
    let session = crate::ServerSession::Shared(state.clone());
    let already_loaded_plugin = session.clone();
    let before = crate::host_bridge::server_identity(&session).unwrap();
    state.set("renamed-session".into());
    let after = crate::host_bridge::server_identity(&already_loaded_plugin).unwrap();
    assert_eq!(before.server_session, "original-session");
    assert_eq!(after.server_session, "renamed-session");
    assert_eq!(before.server_incarnation, after.server_incarnation);
    assert_eq!(before.server_binary, after.server_binary);
}

#[test]
fn session_environment_response_uses_current_identity_and_preserves_config() {
    use prost::Message;
    use std::collections::{BTreeMap, HashSet};
    use yinz_plugin_api::plugin_command::ProtobufGetSessionEnvironmentVariablesResponse;
    use yinz_types::data::{PermissionType, PluginCommand};

    let inherited_name = std::env::var_os("YINZ_SESSION_NAME");
    let state = yinz_runtime::envs::SessionState::new("original-session".into());
    let mut env = crate::test_env::test_env();
    env.server_session = crate::ServerSession::Shared(state.clone());
    env.session_env_vars = BTreeMap::from([
        ("YINZ".into(), "inherited".into()),
        ("YINZ_SESSION_NAME".into(), "stale-snapshot".into()),
        ("CUSTOM_SETTING".into(), "configured-value".into()),
    ]);
    *env.permissions.lock().unwrap() = Some(HashSet::from([
        PermissionType::ReadSessionEnvironmentVariables,
    ]));
    for current in ["original-session", "renamed-session"] {
        state.set(current.into());
        super::run_plugin_command(&mut env, PluginCommand::GetSessionEnvironmentVariables).unwrap();
        let response = ProtobufGetSessionEnvironmentVariablesResponse::decode(
            env.response_sink.take().unwrap().as_slice(),
        )
        .unwrap();
        let vars: BTreeMap<_, _> = response
            .env_vars
            .into_iter()
            .map(|entry| (entry.name, entry.value))
            .collect();
        assert_eq!(vars["YINZ"], "0");
        assert_eq!(vars["YINZ_SESSION_NAME"], current);
        assert_eq!(vars["CUSTOM_SETTING"], "configured-value");
    }
    assert_eq!(std::env::var_os("YINZ_SESSION_NAME"), inherited_name);
}

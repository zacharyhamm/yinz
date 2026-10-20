//! Uniformly operates YINZ* environment variables.
use anyhow::Result;
use std::env::{set_var, var};
use yinz_types::internal_ids::SessionName;

use yinz_types::envs::EnvironmentVariables;

// shortcut: legacy environment writes can race with foreign readers; replace with explicit state and child environments.
pub const YINZ_ENV_KEY: &str = "YINZ";
pub fn get_yinz() -> Result<String> {
    Ok(var(YINZ_ENV_KEY)?)
}
pub fn set_yinz(v: String) {
    unsafe { set_var(YINZ_ENV_KEY, v) };
}

pub const SESSION_NAME_ENV_KEY: &str = "YINZ_SESSION_NAME";

pub fn get_session_name() -> Result<SessionName> {
    Ok(SessionName::new(var(SESSION_NAME_ENV_KEY)?))
}

pub fn set_session_name(v: SessionName) {
    unsafe { set_var(SESSION_NAME_ENV_KEY, v.as_str()) };
}

pub const SOCKET_DIR_ENV_KEY: &str = "YINZ_SOCKET_DIR";
pub fn get_socket_dir() -> Result<String> {
    Ok(var(SOCKET_DIR_ENV_KEY)?)
}

pub fn set_vars(variables: &EnvironmentVariables) {
    for (key, value) in variables.inner() {
        unsafe { set_var(key, value) };
    }
}

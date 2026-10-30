//! Uniformly operates YINZ* environment variables.
use anyhow::Result;
use std::env::var;
use std::sync::{Arc, RwLock};
use yinz_types::internal_ids::SessionName;

pub const YINZ_ENV_KEY: &str = "YINZ";
pub fn get_yinz() -> Result<String> {
    Ok(var(YINZ_ENV_KEY)?)
}

pub const SESSION_NAME_ENV_KEY: &str = "YINZ_SESSION_NAME";

pub fn get_session_name() -> Result<SessionName> {
    Ok(SessionName::new(var(SESSION_NAME_ENV_KEY)?))
}

/// Overrides for a child of a server: inside Yinz, in the session's current name.
///
/// The process environment keeps its startup name after a rename, so every spawn applies these.
pub fn child_session_env(session_name: &SessionName) -> [(&'static str, String); 2] {
    [
        (YINZ_ENV_KEY, "0".to_owned()),
        (SESSION_NAME_ENV_KEY, session_name.to_string()),
    ]
}

pub const SOCKET_DIR_ENV_KEY: &str = "YINZ_SOCKET_DIR";
pub fn get_socket_dir() -> Result<String> {
    Ok(var(SOCKET_DIR_ENV_KEY)?)
}

/// Mutable identity belongs to one server, never to the process environment.
#[derive(Clone, Debug, Default)]
pub struct SessionState(Arc<RwLock<SessionName>>);

impl SessionState {
    pub fn new(name: SessionName) -> Self {
        Self(Arc::new(RwLock::new(name)))
    }

    pub fn current(&self) -> SessionName {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set(&self, name: SessionName) {
        // Replacing a name cannot leave partially updated session data after a panic.
        *self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = name;
    }
}

#[cfg(test)]
#[path = "unit/envs_tests.rs"]
mod tests;

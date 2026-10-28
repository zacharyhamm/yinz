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

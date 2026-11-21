//! Uniformly operates YINZ* environment variables.
use anyhow::Result;
use std::collections::HashMap;
use std::env::var;
use std::sync::{Arc, RwLock};
use yinz_types::internal_ids::{ClientId, SessionName};

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
pub fn child_session_env(session_name: &SessionName) -> [(String, String); 2] {
    [
        (YINZ_ENV_KEY.to_owned(), "0".to_owned()),
        (SESSION_NAME_ENV_KEY.to_owned(), session_name.to_string()),
    ]
}

/// The absolute data directory of the server that spawned this pane.
///
/// Read only to find that server's live update journal, which a hook installed without
/// `--data-dir` could not otherwise locate. Never a general `--data-dir` default: a yinz
/// started inside a pane would then silently share the outer session's database.
pub const SESSION_DATA_DIR_ENV_KEY: &str = "YINZ_SESSION_DATA_DIR";

pub const SOCKET_DIR_ENV_KEY: &str = "YINZ_SOCKET_DIR";
pub fn get_socket_dir() -> Result<String> {
    Ok(var(SOCKET_DIR_ENV_KEY)?)
}

/// Mutable identity belongs to one server, never to the process environment.
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    name: Arc<RwLock<SessionName>>,
    // Standalone components can run without a socket registry.
    attachments: Arc<RwLock<Option<HashMap<u32, ClientId>>>>,
}

impl SessionState {
    pub fn new(name: SessionName) -> Self {
        Self {
            name: Arc::new(RwLock::new(name)),
            ..Default::default()
        }
    }

    pub fn register_attachment(&self, owner: ClientId) -> Option<ClientId> {
        self.attachments
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .get_or_insert_with(HashMap::new)
            .insert(owner.get(), owner)
    }

    pub fn retire_attachment(&self, owner: ClientId) {
        if let Some(owners) = self
            .attachments
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            && owners.get(&owner.get()) == Some(&owner)
        {
            owners.remove(&owner.get());
        }
    }

    /// Resolve a public numeric target once, at the originating command boundary.
    pub fn resolve_client(&self, address: ClientId) -> Option<ClientId> {
        let owners = self
            .attachments
            .read()
            .unwrap_or_else(|error| error.into_inner());
        match owners.as_ref() {
            Some(owners) => owners
                .get(&address.get())
                .copied()
                .filter(|current| address.generation().is_none() || *current == address),
            None => Some(address),
        }
    }

    pub fn owns_attachment(&self, owner: ClientId) -> bool {
        self.resolve_client(owner) == Some(owner)
    }

    /// Hold admission through an owner's commit point. The closure must not reenter this registry.
    pub fn with_attachment<T>(&self, owner: ClientId, commit: impl FnOnce() -> T) -> Option<T> {
        let owners = self
            .attachments
            .read()
            .unwrap_or_else(|error| error.into_inner());
        if owners
            .as_ref()
            .is_some_and(|owners| owners.get(&owner.get()) != Some(&owner))
        {
            return None;
        }
        Some(commit())
    }

    pub fn current(&self) -> SessionName {
        self.name
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set(&self, name: SessionName) {
        // Replacing a name cannot leave partially updated session data after a panic.
        *self
            .name
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = name;
    }
}

#[cfg(test)]
#[path = "unit/envs_tests.rs"]
mod tests;

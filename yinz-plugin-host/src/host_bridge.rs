//! Host-owned identity shared by script runtimes.
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::SessionName;

/// Where the plugin runtime learns this server's current session name.
#[derive(Clone, Debug)]
pub enum ServerSession {
    Shared(yinz_runtime::envs::SessionState),
    Fixed(SessionName),
}

impl ServerSession {
    pub fn current(&self) -> SessionName {
        match self {
            ServerSession::Shared(state) => state.current(),
            ServerSession::Fixed(name) => name.clone(),
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ServerIdentity {
    pub server_binary: String,
    pub server_session: String,
    pub server_incarnation: &'static str,
}

pub(super) fn server_identity(session: &ServerSession) -> Result<ServerIdentity> {
    static INCARNATION: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| uuid::Uuid::new_v4().to_string());
    Ok(ServerIdentity {
        server_binary: yinz_runtime::own_binary::server_binary()
            .context("cannot resolve server executable")?
            .to_string_lossy()
            .into_owned(),
        server_session: session.current().to_string(),
        server_incarnation: INCARNATION.as_str(),
    })
}

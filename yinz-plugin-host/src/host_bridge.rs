//! Host-owned identity shared by script runtimes.
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::SessionName;

/// Where the plugin runtime learns this server's session name.
///
/// The name changes while plugins run: a rename republishes `YINZ_SESSION_NAME` from
/// `OsApi::rename_pty_host_session`, and a plugin instance created after it, or a warp started
/// after it, must use the new name. So the server passes [`ServerSession::Process`], which asks
/// the environment on every call. Tests pass [`ServerSession::Fixed`] instead of writing the
/// variable, which every test in a threaded runner shares.
#[derive(Clone, Debug)]
pub enum ServerSession {
    /// `YINZ_SESSION_NAME` at the moment of the call; empty when it is unset.
    Process,
    /// A name no rename can change.
    Fixed(SessionName),
}

impl ServerSession {
    pub fn current(&self) -> SessionName {
        match self {
            ServerSession::Process => yinz_runtime::envs::get_session_name().unwrap_or_default(),
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

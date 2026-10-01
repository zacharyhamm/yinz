//! Resolve a shell's stable pane reference without trusting inherited session metadata.
use crate::pty_host::{client, protocol, PANE_REF_ENV_KEY};
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::{SessionName, SupervisorId, TerminalId};

pub fn resolve_pane_ref(reference: &str) -> Result<(SessionName, TerminalId)> {
    resolve_with(reference, |id| client::query(&protocol::socket_path(id)))
}

fn resolve_with(
    reference: &str,
    query: impl FnOnce(&SupervisorId) -> Result<protocol::HostStatus>,
) -> Result<(SessionName, TerminalId)> {
    if reference.len() != 12 || !reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid {PANE_REF_ENV_KEY}");
    }
    let reference = SupervisorId::new(reference);
    let status =
        query(&reference).with_context(|| format!("could not resolve pane {reference}"))?;
    if status.host_id != reference {
        bail!(
            "pane reference {reference} resolved to supervisor {}",
            status.host_id
        );
    }
    if !status.attached {
        bail!("pane {reference} is currently between sessions");
    }
    let session = status
        .session_name
        .with_context(|| format!("pane {reference} has no owning session"))?;
    Ok((session, TerminalId::new(status.terminal_id)))
}

pub fn inside_pane() -> bool {
    [
        PANE_REF_ENV_KEY,
        "YINZ",
        "YINZ_PANE_ID",
        "YINZ_SESSION_NAME",
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some())
}

pub fn current_pane() -> Result<Option<(SessionName, TerminalId)>> {
    match std::env::var(PANE_REF_ENV_KEY) {
        Ok(reference) => resolve_pane_ref(&reference).map(Some),
        Err(std::env::VarError::NotPresent) if !inside_pane() => Ok(None),
        Err(_) => bail!("pane has no valid {PANE_REF_ENV_KEY}; pass --session SESSION explicitly"),
    }
}

/// An explicit destination wins even when the caller cannot be identified.
/// Only a caller owned by the destination may supply its default terminal id.
pub fn target(explicit: Option<SessionName>) -> Result<(Option<SessionName>, Option<TerminalId>)> {
    let caller = if explicit.is_some() {
        current_pane().ok().flatten()
    } else {
        current_pane()?
    };
    let session = explicit.or_else(|| caller.as_ref().map(|(session, _)| session.clone()));
    let terminal = caller
        .filter(|(owner, _)| Some(owner) == session.as_ref())
        .map(|(_, id)| id);
    Ok((session, terminal))
}

#[cfg(test)]
#[path = "unit/pane_context_tests.rs"]
mod tests;

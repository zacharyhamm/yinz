//! Fail closed only when a peer or OpenSSH confirms a permanent failure.
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

const MARKER: &str = "YINZ_REMOTE_ERROR ";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapCategory {
    SessionMissing,
    Permission,
    Compatibility,
    Configuration,
    AttachmentRejected,
    Unresponsive,
    Other,
}

#[derive(Debug)]
pub struct BootstrapError {
    pub category: BootstrapCategory,
    pub diagnostic: anyhow::Error,
}
impl fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.diagnostic)
    }
}
impl std::error::Error for BootstrapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.diagnostic.source()
    }
}

/// Kept on stderr so successful bootstrap stdout remains a single endpoint JSON.
pub fn report_bootstrap_error(error: &anyhow::Error) {
    let category = error
        .downcast_ref::<BootstrapError>()
        .map(|error| error.category)
        .unwrap_or(BootstrapCategory::Other);
    // Serializing this enum cannot fail.
    if let Ok(category) = serde_json::to_string(&category) {
        eprintln!("{MARKER}{category}");
    }
    eprintln!("Error: {error:#}");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    Retryable,
    Permanent,
    Cancelled,
}

#[derive(Debug)]
pub(crate) struct ConnectionFailure {
    pub kind: FailureKind,
    pub diagnostic: String,
}

impl ConnectionFailure {
    pub fn classify(error: anyhow::Error, cancelled: &AtomicBool) -> Self {
        let diagnostic = format!("{error:#}");
        let kind = if cancelled.load(Ordering::Acquire) {
            FailureKind::Cancelled
        } else if error.downcast_ref::<BootstrapError>().is_some_and(|error| {
            !matches!(
                error.category,
                BootstrapCategory::Unresponsive | BootstrapCategory::Other
            )
        }) || permanent_diagnostic(&diagnostic)
        {
            FailureKind::Permanent
        } else {
            FailureKind::Retryable
        };
        Self { kind, diagnostic }
    }
}

pub(crate) fn permanent_diagnostic(diagnostic: &str) -> bool {
    if diagnostic.lines().any(|line| {
        line.split_once(MARKER)
            .map(|(_, category)| category)
            .and_then(|category| serde_json::from_str::<BootstrapCategory>(category).ok())
            .is_some_and(|category| {
                !matches!(
                    category,
                    BootstrapCategory::Unresponsive | BootstrapCategory::Other
                )
            })
    }) {
        return true;
    }
    let diagnostic = diagnostic.to_ascii_lowercase();
    [
        "yinz compatibility mismatch",
        "permission denied (",
        "too many authentication failures",
        "host key verification failed",
        "remote host identification has changed",
        "administratively prohibited",
        "yinz: command not found",
        "command not found: yinz",
        "yinz: permission denied",
        "connect failed: permission denied",
        "yinz: not found",
        "yinz: error while loading shared libraries",
        "yinz: cannot execute",
        "yinz: exec format error",
    ]
    .iter()
    .any(|message| diagnostic.contains(message))
}

#[cfg(test)]
#[path = "unit/remote_failure.rs"]
mod tests;

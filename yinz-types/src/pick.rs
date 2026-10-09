//! Structured finder input shared by the host and builtin driver.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PickKind {
    Directory,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PickOptions {
    pub kind: PickKind,
    pub roots: Option<Vec<String>>,
    pub extensions: Option<Vec<String>>,
    pub hidden: Option<bool>,
    pub recursive: Option<bool>,
    pub timeout_ms: Option<u64>,
}

impl PickOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.roots.as_ref().is_some_and(|roots| {
            roots.is_empty()
                || roots
                    .iter()
                    .any(|root| root.is_empty() || root.contains('\0'))
        }) {
            return Err("roots must contain at least one nonempty path without NUL".into());
        }
        if let Some(extensions) = &self.extensions {
            if self.kind == PickKind::Directory {
                return Err("extensions are only supported for file picks".into());
            }
            if extensions.iter().any(|ext| {
                ext.is_empty()
                    || ext.starts_with('.')
                    || ext.contains(['\0', '/', '\\', '*', '?', '[', ']'])
            }) {
                return Err(
                    "extensions must be literal suffixes without leading dots or glob syntax"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PickCommand {
    Start {
        request_id: String,
        options: PickOptions,
    },
    Cancel {
        request_id: String,
        timeout: bool,
    },
    Request,
    Attach {
        pane_id: crate::internal_ids::TerminalId,
    },
    Complete {
        result: Result<Option<String>, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickResult {
    pub request_id: String,
    pub result: Result<Option<String>, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickRequest {
    pub options: PickOptions,
    pub cwd: std::path::PathBuf,
}

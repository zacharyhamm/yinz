//! Portable results from offline plugin authoring checks.
use crate::data::PermissionType;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginCheckMode {
    Syntax,
    Semantic,
    Format,
    Prepare,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCheckReport {
    pub diagnostics: Vec<String>,
    pub outline: Vec<OutlineEntry>,
    pub permissions: Vec<PermissionType>,
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formatted_source: Option<String>,
    /// The entry file's pane declaration, as `PaneDeclaration::describe` words it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
}

/// What one entry of a source analysis is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum OutlineKind {
    /// `(require "./module.scm")`.
    Require,
    /// A literal permission request, with every permission it asks for.
    Permission,
    /// A named function, such as `(define (render rows columns) ...)`.
    Function,
    /// An event handler registered with `yinz-on-event`.
    Handler,
    /// A named value definition.
    Define,
}

/// One line of a source analysis: what is defined or hooked there, and where.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OutlineEntry {
    /// 1-based, as an editor counts them.
    pub line: usize,
    pub kind: OutlineKind,
    /// The entry as an outline prints it: a signature cut after its parameter list, or a call
    /// with the one literal that says what it is about.
    pub text: String,
}

/// Installed Scheme test runner input. `name` is resolved inside the plugin folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PluginTestOptions {
    pub kind: PluginTestKind,
    pub filter: Option<String>,
    pub timeout_ms: u64,
}
impl Default for PluginTestOptions {
    fn default() -> Self {
        Self {
            kind: PluginTestKind::All,
            filter: None,
            timeout_ms: 30_000,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginTestKind {
    Unit,
    Integration,
    All,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginTestResult {
    pub name: String,
    pub kind: PluginTestKind,
    pub status: String,
    pub duration_ms: u64,
    pub error: Option<String>,
    pub output: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginTestReport {
    pub tests: Vec<PluginTestResult>,
}
impl PluginTestReport {
    pub fn passed(&self) -> bool {
        !self.tests.is_empty() && self.tests.iter().all(|test| test.status == "passed")
    }
}

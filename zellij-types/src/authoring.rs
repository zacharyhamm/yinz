//! Portable results from offline plugin authoring checks.
use crate::data::PermissionType;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginCheckMode {
    Syntax,
    Semantic,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCheckReport {
    pub diagnostics: Vec<String>,
    pub outline: Vec<OutlineEntry>,
    pub permissions: Vec<PermissionType>,
    pub warnings: Vec<String>,
}

/// What one entry of an source analysis is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum OutlineKind {
    /// `require "name"`.
    Require,
    /// `request_permission { ... }`, with every name it asks for.
    Permission,
    /// A named function in either spelling: `function M.load(config)`, `local function tick()`
    /// or `M.render = function(rows, cols)`.
    Function,
    /// `on "event"`: a handler registered with `zellij.on`.
    Handler,
    /// `data.define "entity"`.
    Define,
    /// A hocus extension's registration message.
    Registration,
    /// One tool that registration offers hocus.
    Tool,
}

/// One line of an source analysis: what is defined or hooked there, and where.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OutlineEntry {
    /// 1-based, as an editor counts them.
    pub line: usize,
    pub kind: OutlineKind,
    /// The entry as an outline prints it: a signature cut after its parameter list, or a call
    /// with the one literal that says what it is about.
    pub text: String,
}

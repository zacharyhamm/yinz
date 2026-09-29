//! Named operations owned by background services, independent of the chat UI.
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{io::Read, path::PathBuf};
use yinz_types::input::actions::Action;

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum Operations {
    /// Discover host, session, caller, project and background services.
    Context {
        #[clap(long)]
        pane_id: Option<u32>,
    },
    /// List live agents and their observed state.
    AgentList {
        #[clap(long)]
        project: Option<String>,
    },
    /// Register a terminal pane as an agent.
    AgentRegister { pane_id: u32, kind: String },
    /// Launch through the shared planner. --file accepts all AgentSpawnSpec fields.
    AgentSpawn {
        #[clap(long, required_unless_present = "file")]
        kind: Option<String>,
        #[clap(long)]
        file: Option<PathBuf>,
        #[clap(long)]
        project: Option<String>,
        #[clap(long)]
        cwd: Option<PathBuf>,
        /// SSH destination. Use an existing remote worktree and explicit --cwd.
        #[clap(long, conflicts_with = "firecracker")]
        remote_host: Option<String>,
        #[clap(long)]
        model: Option<String>,
        #[clap(long)]
        effort: Option<String>,
        #[clap(long, conflicts_with = "prompt_file")]
        prompt: Option<String>,
        #[clap(long)]
        prompt_file: Option<PathBuf>,
        #[clap(long)]
        permission_mode: Option<String>,
        #[clap(long)]
        branch: Option<String>,
        #[clap(long)]
        base: Option<String>,
        #[clap(long)]
        worktree: Option<bool>,
        #[clap(long)]
        worktree_dir: Option<String>,
        #[clap(long)]
        firecracker: Option<String>,
        #[clap(long)]
        firecracker_image: Option<String>,
        #[clap(long)]
        reuse: Option<bool>,
        #[clap(long)]
        allow_outside_worktree: Option<bool>,
    },
    /// Send text to an explicit agent pane, optionally pressing Enter.
    AgentSend {
        pane_id: u32,
        #[clap(long, conflicts_with = "file", required_unless_present = "file")]
        text: Option<String>,
        #[clap(long)]
        file: Option<PathBuf>,
        #[clap(long)]
        enter: bool,
    },
    /// Send an interrupt or Escape, or close an explicit agent pane.
    AgentControl {
        pane_id: u32,
        #[clap(value_parser = ["interrupt", "escape", "close"])]
        action: String,
    },
    /// Assign an existing project to an explicit tab.
    ProjectAssign {
        project: String,
        #[clap(long)]
        tab_id: u32,
    },
    /// Read a project's workflow configuration and its exact source.
    ProjectConfigRead { project: String },
    /// Supply {source, expected}; expected is the previously read source or null.
    ProjectConfigWrite {
        project: String,
        #[clap(long)]
        file: PathBuf,
    },
    /// Open an existing project in a new tab.
    ProjectOpen { project: String },
    /// Swap two unambiguously adjacent tiled panes.
    PaneSwap { first: String, second: String },
    /// Move an explicit pane to an existing tab.
    PaneMoveToTab {
        pane_id: String,
        #[clap(long)]
        tab_id: usize,
    },
    /// Rerun a held command pane.
    PaneRerun { pane_id: u32 },
    /// Pin an explicit floating pane.
    PanePin { pane_id: String },
    /// Unpin an explicit floating pane.
    PaneUnpin { pane_id: String },
}

impl Operations {
    pub fn action(&self, stdin: &mut impl Read) -> Result<Action, String> {
        let mut request = serde_json::to_value(self).map_err(|e| e.to_string())?;
        let args = request["args"]
            .as_object_mut()
            .ok_or("invalid operation arguments")?;
        args.retain(|_, value| !value.is_null());
        let read = |path: &str, stdin: &mut dyn Read| -> Result<String, String> {
            if path == "-" {
                let mut text = String::new();
                stdin.read_to_string(&mut text).map_err(|e| e.to_string())?;
                Ok(text)
            } else {
                std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
            }
        };
        if args.get("file").and_then(serde_json::Value::as_str) == Some("-")
            && args.get("prompt_file").and_then(serde_json::Value::as_str) == Some("-")
        {
            return Err("stdin can supply either the spawn spec or the prompt, not both".into());
        }
        if let Some(file) = args.remove("file") {
            let contents = read(file.as_str().ok_or("invalid file path")?, stdin)?;
            if matches!(
                self,
                Self::AgentSpawn { .. } | Self::ProjectConfigWrite { .. }
            ) {
                let mut spec: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(&contents).map_err(|e| {
                        format!("expected operation arguments as a JSON object: {e}")
                    })?;
                spec.extend(std::mem::take(args));
                *args = spec;
            } else {
                args.insert("text".into(), json!(contents));
            }
        }
        if let Some(file) = args.remove("prompt_file") {
            args.insert(
                "prompt".into(),
                json!(read(file.as_str().ok_or("invalid prompt file")?, stdin)?),
            );
        }
        if matches!(self, Self::AgentSpawn { .. }) {
            args.retain(|key, value| {
                !matches!(key.as_str(), "cwd" | "project" | "remote_host") || !value.is_null()
            });
            if !args.contains_key("remote_host") {
                if let Some(cwd) = args.get("cwd").and_then(serde_json::Value::as_str) {
                    let cwd = std::path::absolute(cwd).map_err(|error| error.to_string())?;
                    args.insert("cwd".into(), json!(cwd));
                } else if !args.contains_key("cwd")
                    && !args.contains_key("project")
                    && (args
                        .get("firecracker")
                        .and_then(serde_json::Value::as_str)
                        .is_none_or(|vm| vm.trim().is_empty())
                        || args
                            .get("branch")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|branch| !branch.trim().is_empty()))
                {
                    args.insert(
                        "cwd".into(),
                        json!(std::env::current_dir().map_err(|error| error.to_string())?),
                    );
                }
            }
        }
        request["structured_reply"] = json!(true);
        Ok(crate::notes_todos::pipe_action("agent_action", request))
    }
}

#[cfg(test)]
#[path = "unit/operations.rs"]
mod tests;

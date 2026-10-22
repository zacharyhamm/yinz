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
        /// The terminal pane to describe as the caller, such as terminal_7 or 7.
        #[clap(long, value_parser = crate::args::parse_terminal_pane_number)]
        pane_id: Option<u32>,
    },
    /// Assign an existing project to an explicit tab.
    ProjectAssign {
        project: String,
        #[clap(long)]
        tab_id: u32,
    },
    /// Read a project's agent-exit configuration and its exact source.
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
    PaneRerun {
        /// The terminal pane, such as terminal_7 or 7.
        #[clap(value_parser = crate::args::parse_terminal_pane_number)]
        pane_id: u32,
    },
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
        if let Some(file) = args.remove("file") {
            let contents = read(file.as_str().ok_or("invalid file path")?, stdin)?;
            let mut supplied: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&contents)
                    .map_err(|e| format!("expected operation arguments as a JSON object: {e}"))?;
            supplied.extend(std::mem::take(args));
            *args = supplied;
        }
        request["structured_reply"] = json!(true);
        Ok(crate::projects::pipe_action("agent_action", request))
    }
}

#[cfg(test)]
#[path = "unit/operations.rs"]
mod tests;

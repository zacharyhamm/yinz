use clap::Subcommand;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum AgentArchive {
    /// List agent run headers with pagination across the archive.
    #[clap(name = "runs")]
    AgentRuns {
        #[clap(long)]
        project: Option<String>,
        #[clap(long)]
        host_id: Option<String>,
        #[clap(long)]
        run_session: Option<String>,
        #[clap(long)]
        kind: Option<String>,
        #[clap(long)]
        since_ms: Option<i64>,
        #[clap(long)]
        active: Option<bool>,
        #[clap(long, default_value_t = 20)]
        limit: usize,
        #[clap(long, default_value_t = 0)]
        offset: usize,
        /// Accepted for symmetry with the live commands; archive output is always JSON.
        #[clap(long, hide = true)]
        #[serde(skip)]
        json: bool,
    },
    /// Read an exact run and a bounded page of its timeline.
    #[clap(name = "run")]
    AgentRun {
        run_id: String,
        #[clap(long, conflicts_with = "from_revision")]
        revision: Option<u64>,
        #[clap(long)]
        from_revision: Option<u64>,
        #[clap(long, default_value_t = 20)]
        limit: usize,
        /// Accepted for symmetry with the live commands; archive output is always JSON.
        #[clap(long, hide = true)]
        #[serde(skip)]
        json: bool,
    },
    /// Search recorded conversation and retained tool names, arguments, output, and errors.
    #[clap(name = "history-search")]
    AgentHistorySearch {
        query: String,
        /// Use case-sensitive Rust regex syntax (inline flags supported), scanning newest entries first.
        #[clap(long)]
        #[serde(default)]
        regex: bool,
        #[clap(long)]
        run_id: Option<String>,
        #[clap(long)]
        project: Option<String>,
        #[clap(long)]
        host_id: Option<String>,
        #[clap(long)]
        run_session: Option<String>,
        #[clap(long)]
        kind: Option<String>,
        #[clap(long)]
        since_ms: Option<i64>,
        #[clap(long)]
        active: Option<bool>,
        #[clap(long, default_value_t = 20)]
        limit: usize,
        /// Accepted for symmetry with the live commands; archive output is always JSON.
        #[clap(long, hide = true)]
        #[serde(skip)]
        json: bool,
    },
    /// Follow recorded predecessor links from an exact run ID.
    #[clap(name = "predecessors")]
    AgentPredecessors {
        run_id: String,
        /// Accepted for symmetry with the live commands; archive output is always JSON.
        #[clap(long, hide = true)]
        #[serde(skip)]
        json: bool,
    },
}

impl AgentArchive {
    pub fn json(&self) -> bool {
        match self {
            AgentArchive::AgentRuns { json, .. }
            | AgentArchive::AgentRun { json, .. }
            | AgentArchive::AgentHistorySearch { json, .. }
            | AgentArchive::AgentPredecessors { json, .. } => *json,
        }
    }

    pub fn action(&self) -> Result<yinz_types::input::actions::Action, String> {
        let mut request = serde_json::to_value(self).map_err(|e| e.to_string())?;
        request["args"]
            .as_object_mut()
            .ok_or("invalid archive arguments")?
            .retain(|_, value| !value.is_null());
        request["structured_reply"] = true.into();
        Ok(crate::projects::pipe_action("store_query", request))
    }
}

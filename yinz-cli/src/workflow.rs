use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum WorkflowOperation {
    Search {
        query: Option<String>,
    },
    Get {
        workflow: String,
    },
    Schema {
        workflow: Option<String>,
    },
    Create {
        name: String,
    },
    /// Supply {expected, definition}; expected is the complete result of get.
    Edit {
        workflow: String,
        #[clap(long)]
        file: PathBuf,
    },
    /// Supply {expected}; stale definitions are refused.
    Delete {
        workflow: String,
        #[clap(long)]
        file: PathBuf,
    },
    Validate {
        workflow: String,
    },
    Evaluate {
        workflow: String,
        #[clap(long)]
        file: Option<PathBuf>,
    },
    Active,
    History {
        #[clap(long)]
        workflow: Option<String>,
        #[clap(long)]
        query: Option<String>,
        #[clap(long)]
        filter: Option<String>,
    },
    RunGet {
        run: String,
    },
    Logs {
        run: String,
        #[clap(long)]
        file: Option<PathBuf>,
    },
    Reports {
        #[clap(long)]
        file: Option<PathBuf>,
    },
    Report {
        name: String,
        #[clap(long)]
        file: Option<PathBuf>,
    },
    /// Supply a data query including its predicate and optional limit.
    Data {
        #[clap(long)]
        file: PathBuf,
    },
    Models {
        query: Option<String>,
    },
    Model {
        model: String,
    },
    ModelTypes {
        query: Option<String>,
    },
    ModelType {
        #[serde(rename = "type")]
        model_type: String,
    },
    ModelMethod {
        model: String,
        method: String,
    },
    Resume {
        run: String,
        #[clap(long)]
        file: Option<PathBuf>,
    },
    Approvals,
    Approve {
        run: String,
        #[clap(long)]
        file: Option<PathBuf>,
    },
    Reject {
        run: String,
        #[clap(long)]
        file: Option<PathBuf>,
    },
    CancelStep {
        run: String,
        work: String,
        key: String,
    },
}

impl WorkflowOperation {
    pub fn request(&self, directory: &Path, stdin: &mut impl Read) -> Result<Value, String> {
        let value = serde_json::to_value(self).map_err(|error| error.to_string())?;
        let mut args = value
            .get("args")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        args.retain(|_, value| !value.is_null());
        if let Some(Value::String(file)) = args.remove("file") {
            let contents = if file == "-" {
                let mut contents = String::new();
                stdin
                    .read_to_string(&mut contents)
                    .map_err(|error| error.to_string())?;
                contents
            } else {
                std::fs::read_to_string(&file)
                    .map_err(|error| format!("cannot read {file}: {error}"))?
            };
            let mut supplied: serde_json::Map<String, Value> = serde_json::from_str(&contents)
                .map_err(|error| {
                    format!("expected workflow arguments as a JSON object: {error}")
                })?;
            supplied.extend(args);
            args = supplied;
        }
        Ok(
            json!({"tool":format!("workflow_{}", value["op"].as_str().ok_or("invalid workflow operation")?), "args":args,"directory":directory}),
        )
    }
}

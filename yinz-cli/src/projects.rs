//! Named shell access to the existing shared-store operations.
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use yinz_types::input::actions::Action;

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum Projects {
    /// Rename a project.
    ProjectRename { name: String, new_name: String },
    /// Merge duplicate project identities, preserving the target's settings.
    ProjectMerge { source: String, target: String },
    /// List remembered projects.
    ProjectList,
    /// Remember a project name and absolute root.
    ProjectRemember {
        name: String,
        #[clap(long)]
        root: String,
        #[clap(long)]
        worktree_dir: Option<String>,
    },
    /// Forget a project name.
    ProjectForget { name: String },
}

impl Projects {
    pub fn request(&self) -> Result<Value, String> {
        let mut request = serde_json::to_value(self).map_err(|e| e.to_string())?;
        if request.get("args").is_none() {
            request["args"] = json!({});
        }
        let args = request["args"]
            .as_object_mut()
            .ok_or("invalid store arguments")?;
        args.retain(|_, value| !value.is_null());
        request["structured_reply"] = json!(true);
        Ok(request)
    }

    pub fn action(&self) -> Result<Action, String> {
        let request = self.request()?;
        let query = matches!(self, Self::ProjectList);
        Ok(pipe_action(
            if query { "store_query" } else { "store_mutate" },
            request,
        ))
    }
}

pub fn pipe_action(name: &str, request: Value) -> Action {
    Action::CliPipe {
        focus_plugin: false,
        pipe_id: uuid::Uuid::new_v4().to_string(),
        name: Some(name.into()),
        payload: Some(request.to_string()),
        plugin: None,
        args: None,
        configuration: None,
        launch_new: false,
        skip_cache: false,
        floating: None,
        in_place: None,
        cwd: None,
        pane_title: None,
    }
}

#[cfg(test)]
#[path = "unit/projects.rs"]
mod tests;

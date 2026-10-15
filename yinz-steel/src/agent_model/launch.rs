//! Pure launch planning; the guest performs the resulting host commands.
use super::{decode, encode};
use agent_proto::launch;
use anyhow::{bail, Result};
use serde_json::Value;
use yinz_types::command::PaneTarget;

pub(super) fn run(op: &str, args: Value) -> Result<Value> {
    match op {
        "Offered" => encode(launch::offered()),
        "BuiltinOffered" => encode(launch::OFFERED),
        "Defaults" => encode(
            serde_json::json!({"base":launch::DEFAULT_BASE,"worktree_prefix":launch::WORKTREE_PREFIX}),
        ),
        "EffortOptions" | "PermissionModes" => {
            let (kind,): (String,) = decode(args)?;
            encode(if op == "EffortOptions" {
                launch::effort_options(&kind)
            } else {
                launch::permission_modes(&kind)
            })
        },
        "Plan" => {
            let (spec,): (launch::Spec,) = decode(args)?;
            encode(launch::plan(&spec))
        },
        "Target" => {
            let (host, bubblewrap): (Option<String>, Option<String>) = decode(args)?;
            encode(match bubblewrap {
                Some(root) => PaneTarget::bubblewrap(std::path::Path::new(&root)),
                None => Ok(PaneTarget::parse_display(host.as_deref())),
            })
        },
        "CommandInContext" => {
            let (kind, cwd, target): (String, Option<std::path::PathBuf>, PaneTarget) =
                decode(args)?;
            encode(launch::command_in_context(&kind, cwd, target))
        },
        "ResumeCommand" => {
            let (kind, session_id, cwd, host, bubblewrap): (
                String,
                String,
                Option<String>,
                Option<String>,
                Option<String>,
            ) = decode(args)?;
            encode(launch::resume_command(
                &kind,
                &session_id,
                cwd.as_deref(),
                host.as_deref(),
                bubblewrap.as_deref(),
            ))
        },
        _ => bail!("unknown launch model operation {op}"),
    }
}

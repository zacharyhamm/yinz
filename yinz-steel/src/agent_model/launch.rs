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
        "ResumeRunCommand" => {
            let (run, host_id): (
                agent_proto::agents::AgentRunHeader,
                yinz_types::internal_ids::HostId,
            ) = decode(args)?;
            encode(launch::resume_directory(&run, &host_id).and_then(|cwd| {
                launch::resume_command(
                    &run.kind,
                    &run.session_id,
                    Some(cwd),
                    run.host.as_deref(),
                    run.bubblewrap.as_deref(),
                )
            }))
        },
        _ => bail!("unknown launch model operation {op}"),
    }
}

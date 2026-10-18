//! Bind existing pure project models; the Scheme caller owns reads and transactions.
use super::{decode as args, encode as value};
use agent_proto::{location::Location, projects};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use yinz_types::data::DataRows;

fn change(change: projects::Change) -> Value {
    json!({"project": change.project, "replaced": change.replaced, "mutations": change.mutations})
}

pub(super) fn run(op: &str, input: Value) -> Result<Value> {
    match op {
        "FromRows" => {
            let (rows,): (DataRows,) = args(input)?;
            value(projects::from_rows(&rows))
        },
        "RowIds" => {
            let (rows,): (DataRows,) = args(input)?;
            value(projects::row_ids(&rows))
        },
        "ParseLegacy" => {
            let (bytes,): (Vec<u8>,) = args(input)?;
            value(projects::parse_legacy(&bytes))
        },
        "ToMutation" => {
            let (project,): (projects::Project,) = args(input)?;
            value(projects::to_mutation(&project))
        },
        "DiffMutations" => {
            let (before, after): (projects::Registry, projects::Registry) = args(input)?;
            value(projects::diff_mutations(&before, &after))
        },
        "Discover" => {
            let (mut registry, repositories): (projects::Registry, Vec<projects::Repository>) =
                args(input)?;
            let discovery = projects::discover(&mut registry, &repositories);
            Ok(
                json!({"registry": registry, "changed": discovery.changed(), "root_moves": discovery.root_moves}),
            )
        },
        "PlanRemember" => {
            let (rows, name, root, worktree_dir): (
                DataRows,
                String,
                Option<String>,
                Option<String>,
            ) = args(input)?;
            value(projects::plan_remember(
                &rows,
                &name,
                root.as_deref(),
                worktree_dir.as_deref(),
            ))
        },
        "PlanRename" | "PlanMerge" => {
            let (rows, from, to): (DataRows, String, String) = args(input)?;
            value(
                if op == "PlanRename" {
                    projects::plan_rename(&rows, &from, &to)
                } else {
                    projects::plan_merge(&rows, &from, &to)
                }
                .map(change),
            )
        },
        "PlanAdopt" => {
            let (rows, project): (DataRows, projects::Project) = args(input)?;
            value(projects::plan_adopt(&rows, project).map(|(adopted, mutations)| {
                json!({"project": adopted.project, "replaced": adopted.replaced, "mutations": mutations})
            }))
        },
        "AssertProjects" => {
            let (rows, column, values): (DataRows, String, Vec<String>) = args(input)?;
            value(projects::assert_projects(
                &rows,
                &column,
                &values.iter().map(String::as_str).collect::<Vec<_>>(),
            ))
        },
        "DeleteMutation" => {
            let (name,): (String,) = args(input)?;
            value(projects::delete_mutation(&name))
        },
        "Forgotten" => {
            let (project,): (projects::Project,) = args(input)?;
            value(projects::forgotten(&project))
        },
        "ForCwd" => {
            let (registry, cwd): (projects::Registry, Location) = args(input)?;
            value(projects::for_cwd(&registry, &cwd))
        },
        "ForRepository" => {
            let (registry, repository): (projects::Registry, projects::Repository) = args(input)?;
            value(projects::for_repository(&registry, &repository))
        },
        "RegisteredForCwd" | "ProjectForCwd" | "LabelForCwd" => {
            let (probed, registry, cwd): (projects::Probed, projects::Registry, Location) =
                args(input)?;
            if op == "ProjectForCwd" {
                value(projects::project_for_cwd(&probed, &registry, &cwd))
            } else if op == "RegisteredForCwd" {
                value(projects::registered_for_cwd(&probed, &registry, &cwd))
            } else {
                value(projects::label_for_cwd(&probed, &registry, &cwd))
            }
        },
        "RepositoryDetails" => {
            let (repository,): (projects::Repository,) = args(input)?;
            Ok(
                json!({"is_checkout": repository.is_checkout(), "checkout_name": repository.checkout_name()}),
            )
        },
        "OriginIdentity" => {
            let (origin,): (String,) = args(input)?;
            value(projects::origin_identity(&origin))
        },
        "ConfigurationRoot" => {
            let (common, fallback): (Option<String>, String) = args(input)?;
            value(
                common
                    .as_deref()
                    .map(std::path::Path::new)
                    .filter(|path| path.file_name().is_some_and(|name| name == ".git"))
                    .and_then(std::path::Path::parent)
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or(fallback),
            )
        },
        "RepositoryIdentity" => {
            let (repository,): (projects::Repository,) = args(input)?;
            value(
                repository
                    .provider_identity
                    .as_ref()
                    .map(|id| format!("provider:{id}"))
                    .or_else(|| {
                        repository
                            .remote
                            .as_deref()
                            .filter(|remote| !remote.is_empty())
                            .map(|remote| format!("remote:{}", projects::origin_identity(remote)))
                            .or_else(|| {
                                repository
                                    .common_dir
                                    .as_ref()
                                    .map(|dir| format!("git:{:?}:{dir}", repository.host))
                            })
                    }),
            )
        },
        _ => bail!("unknown project model operation {op}"),
    }
}

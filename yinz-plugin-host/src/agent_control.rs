//! Shared automation entry point. Waits run outside the plugin executor.
use super::*;
use yinz_types::agent::control::{AgentOutcome, AgentRequest, AgentResponse, MAX_TIMEOUT_MS};
use yinz_types::plugin_data::DataValue;

pub fn control_agent(
    mut request: AgentRequest,
    client: crate::ClientId,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> AgentResponse {
    // Establish the retry identity before routing: the screen may accept the prompt even
    // when its reply never arrives, so a caller must still be able to retrieve that turn.
    let submission_id = match &mut request {
        AgentRequest::Prompt { submission_id, .. } => Some(
            submission_id
                .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
                .clone(),
        ),
        AgentRequest::WaitTurn { submission_id, .. } => Some(submission_id.clone()),
        _ => None,
    };
    let result = (|| -> Result<AgentResponse, String> {
        if let AgentRequest::Spawn {
            spec,
            cwd,
            project,
            remote_host,

            placement,
        } = request
        {
            let mut spawned = match spawn_agent_for_control(
                placement,
                spec,
                cwd,
                project,
                remote_host,
                client,
                senders,
                data,
            ) {
                Ok(spawned) => spawned,
                Err(error) => return Ok(error.into_response()),
            };
            if spawned.reused {
                spawned.occupied_by = occupants(senders, &spawned);
            }
            let mut response = register_spawned_agent(senders, spawned);
            if response.message.is_none() {
                response.message = response.spawned.as_ref().and_then(spawn_notes);
            }
            return Ok(response);
        }
        let timeout = match &request {
            AgentRequest::Prompt { timeout_ms, .. }
            | AgentRequest::Wait { timeout_ms, .. }
            | AgentRequest::WaitTurn { timeout_ms, .. } => (*timeout_ms).min(MAX_TIMEOUT_MS),
            _ => 30_000,
        };
        let project = match &request {
            AgentRequest::List { project } => project.clone(),
            _ => None,
        };
        let (reply, receiver) = std::sync::mpsc::channel();
        senders
            .send_to_screen(ScreenInstruction::AgentControlReply(request, reply))
            .map_err(|e| format!("cannot route agent request: {e:#}"))?;
        let mut response =
            match receiver.recv_timeout(std::time::Duration::from_millis(timeout + 2000)) {
                Ok(response) => response,
                Err(error) => {
                    let mut response = AgentResponse::new(AgentOutcome::TimedOut);
                    response.message = Some(format!(
                        "agent request outcome unknown; inspect before retrying: {error}"
                    ));
                    return Ok(response);
                },
            };
        if let Some(project) = project {
            filter_project(&mut response, &project, data, |pane_id, cwd| {
                let (reply, receiver) = crossbeam::channel::bounded(1);
                if let Err(error) = senders.send_to_pty(PtyInstruction::GetPaneGitProbe {
                    pane_id,
                    cwd: cwd.to_owned(),
                    refresh_token: None,
                    response_channel: reply,
                }) {
                    return yinz_types::data::GetPaneGitProbeResponse::Err(error.to_string());
                }
                receiver
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap_or_else(|error| {
                        yinz_types::data::GetPaneGitProbeResponse::Err(error.to_string())
                    })
            })?;
        }
        Ok(response)
    })();
    let mut response = result.unwrap_or_else(|error| {
        let mut response = AgentResponse::new(AgentOutcome::Rejected);
        response.message = Some(error);
        response
    });
    if response.submission_id.is_none() {
        response.submission_id = submission_id;
    }
    response
}

pub(crate) fn agent_event_projects(
    agents: Vec<yinz_types::agent::control::AgentStatus>,
    projects: Vec<String>,
    data: &DataAccess,
    senders: &ThreadSenders,
) -> yinz_bus::screen::AgentEventProjectMemberships {
    let mut memberships = yinz_bus::screen::AgentEventProjectMemberships::new();
    for project in projects {
        let mut response = AgentResponse::new(AgentOutcome::Ok);
        response.agents = agents.clone();
        let result = filter_project(&mut response, &project, data, |pane_id, cwd| {
            let (reply, receiver) = crossbeam::channel::bounded(1);
            if let Err(error) = senders.send_to_pty(PtyInstruction::GetPaneGitProbe {
                pane_id,
                cwd: cwd.to_owned(),
                refresh_token: None,
                response_channel: reply,
            }) {
                return yinz_types::data::GetPaneGitProbeResponse::Err(error.to_string());
            }
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap_or_else(|error| {
                    yinz_types::data::GetPaneGitProbeResponse::Err(error.to_string())
                })
        })
        .map(|pending| {
            (!pending).then(|| {
                response
                    .agents
                    .into_iter()
                    .map(|agent| agent.pane_id)
                    .collect()
            })
        });
        memberships.insert(project, result);
    }
    memberships
}

pub(super) fn projects(data: &DataAccess) -> Result<yinz_types::data::DataRows, String> {
    data.plugin_data()
        .fetch(
            project_data::store::SHARED,
            &FetchRequest::entity("project").limit(project_data::store::LIST_FETCH_LIMIT),
        )
        .or_else(|error| match error {
            DataError::UnknownEntity(_) => Ok(Default::default()),
            error => Err(error),
        })
        .map_err(|e| e.to_string())
        .and_then(|rows| {
            if rows.truncated {
                Err("project registry is incomplete".into())
            } else {
                Ok(rows)
            }
        })
}

// Returns whether any required remote probe is still pending.
fn filter_project(
    response: &mut AgentResponse,
    name: &str,
    data: &DataAccess,
    mut probe_of: impl FnMut(PaneId, &str) -> yinz_types::data::GetPaneGitProbeResponse,
) -> Result<bool, String> {
    let rows = projects(data)?;
    let registry = project_data::projects::from_rows(&rows);
    let project = registry
        .get(name)
        .ok_or("unknown project; use project-list")?;
    let ids = project_data::projects::row_ids(&rows);
    let project_id = ids.get(name).ok_or("project has no identity")?;
    let mut assignments = BTreeMap::new();
    for status in response
        .agents
        .iter()
        .filter(|status| status.tab_id.is_some())
    {
        let key = (status.host_id.clone(), status.session.clone());
        if let std::collections::btree_map::Entry::Vacant(entry) = assignments.entry(key) {
            let owner = project_data::store::Owner::new(
                status.host_id.clone(),
                status.session.clone().into(),
            );
            let rows = data.plugin_data().fetch(
                project_data::store::SHARED,
                &project_data::store::owned_rows_request("agent_assignment", &owner)
                    .limit(project_data::store::LIST_FETCH_LIMIT),
            );
            entry.insert(match rows {
                Ok(rows) if !rows.truncated => Some(rows),
                Ok(_) => return Err("project assignments are incomplete".into()),
                Err(DataError::UnknownEntity(_)) => None,
                Err(error) => return Err(error.to_string()),
            });
        }
    }
    let mut pending = false;
    response.agents.retain(|status| {
        let rows = assignments
            .get(&(status.host_id.clone(), status.session.clone()))
            .and_then(Option::as_ref);
        if let (Some(rows), Some(tab)) = (rows, status.tab_id) {
            if let Some(row) = rows
                .rows
                .iter()
                .find(|row| rows.get(row, "tab_id") == Some(&DataValue::Int(tab.get() as i64)))
            {
                return rows.get(row, "project_id").is_some_and(|id| {
                    id == &DataValue::Int(project_id.get())
                        || project
                            .replaced_ids
                            .iter()
                            .any(|old| id == &DataValue::Int(old.get()))
                });
            }
        }
        let cwd = match &status.target {
            yinz_types::command::PaneTarget::Ssh { host } => {
                let Some(cwd) = status.cwd.as_deref() else {
                    return false;
                };
                let probe = match probe_of(status.pane_id, cwd) {
                    yinz_types::data::GetPaneGitProbeResponse::Ok(probe) => probe,
                    yinz_types::data::GetPaneGitProbeResponse::Pending
                    | yinz_types::data::GetPaneGitProbeResponse::Err(_) => {
                        pending = true;
                        return false;
                    },
                };
                if probe.host != *host || probe.cwd != cwd {
                    return false;
                }
                let project_data::git_probe::ProbeOutcome::Checkout(mut repository) =
                    project_data::git_probe::repository(
                        probe.exit_code,
                        &probe.metadata,
                        &probe.config,
                    )
                else {
                    return false;
                };
                repository.host = Some(probe.host);
                return project_data::projects::for_repository(&registry, &repository)
                    .is_some_and(|owner| owner.name == project.name);
            },
            _ => status.cwd.as_deref(),
        };
        cwd.is_some_and(|cwd| {
            let probe = yinz_process::pty_host::remote::status::probe_git(cwd);
            let project_data::git_probe::ProbeOutcome::Checkout(repository) =
                project_data::git_probe::repository(
                    probe.exit_code,
                    &probe.metadata,
                    &probe.config,
                )
            else {
                return false;
            };
            let location = project_data::location::Location::local(cwd);
            let probed = BTreeMap::from([(location.clone(), repository)]);
            project_data::projects::project_for_cwd(&probed, &registry, &location)
                .is_some_and(|owner| owner.name == project.name)
        })
    });
    Ok(pending)
}

pub(super) fn prepare_automation_agent(
    spec: &mut AgentSpawnSpec,
    cwd: Option<PathBuf>,
    project: Option<String>,
    remote_host: Option<String>,
    data: &DataAccess,
) -> Result<PreparedAgent, AgentSpawnError> {
    if spec.bubblewrap && remote_host.is_some() {
        return Err("Bubblewrap cannot be combined with SSH".into());
    }
    let guidance = || {
        let kinds = yinz_types::agent::KINDS
            .iter()
            .map(|kind| (*kind).to_owned())
            .chain(
                yinz_types::agent::provider::providers()
                    .into_iter()
                    .map(|provider| provider.kind),
            )
            .collect::<Vec<_>>();
        format!("Use --kind with a recognized kind: {}. Alternatively, use --file PATH (or --file - for stdin) with a JSON object containing a recognized executable in command and optional args.", kinds.join(", "))
    };
    if [&spec.kind, &spec.command]
        .iter()
        .all(|value| value.as_deref().is_none_or(|value| value.trim().is_empty()))
    {
        return Err(format!("agent spawn requires a kind or command. {}", guidance()).into());
    }
    let (mut command, _) = yinz_runtime::agent::launch::plan(spec)?;
    if yinz_types::agent::kind_from_program_and_args(&command.path, &command.args).is_none() {
        return Err(format!(
            "agent spawn requires a recognized agent command. {}",
            guidance()
        )
        .into());
    }
    spec.env
        .insert(yinz_types::agent::AGENT_PANE_ENV.into(), "1".into());
    if let Some(host) = remote_host {
        if project.is_some()
            || spec.branch.is_some()
            || spec.base.is_some()
            || spec.worktree_dir.is_some()
            || spec.worktree.is_some()
        {
            return Err("SSH agents use an existing remote checkout: pass cwd without project, branch or worktree options".into());
        }
        if spec.allow_outside_worktree != Some(true) {
            return Err("the local worktree guard cannot inspect SSH paths; verify the remote worktree and explicitly allow outside worktree".into());
        }
        let cwd = cwd
            .filter(|cwd| cwd.is_absolute())
            .ok_or("SSH cwd must be absolute")?;
        command = yinz_runtime::agent::launch::plan(spec)?.0;
        // SSH uses the remote installation, never the local extension placeholder.
        yinz_runtime::agent::pi::strip_extension(&mut command.args);
        command.target = yinz_types::command::PaneTarget::ssh(&host)?;
        command.cwd = Some(cwd.clone());
        return Ok(PreparedAgent {
            command,
            cwd,
            branch: None,
            requested_branch: None,
            reused: false,
        });
    }
    let registry = project
        .as_ref()
        .map(|_| projects(data).map(|rows| project_data::projects::from_rows(&rows)))
        .transpose()?;
    let project = project
        .as_ref()
        .map(|name| {
            registry
                .as_ref()
                .and_then(|r| r.get(name))
                .ok_or("unknown project; use project-list")
        })
        .transpose()?;
    let cwd = cwd
        .map(|cwd| {
            cwd.canonicalize()
                .map_err(|e| format!("cannot resolve agent directory: {e}"))
        })
        .transpose()?;
    if let (Some(project), Some(cwd)) = (project, &cwd) {
        let root = PathBuf::from(&project.root)
            .canonicalize()
            .map_err(|e| format!("cannot resolve project directory: {e}"))?;
        if !cwd.starts_with(root) {
            return Err(format!("cwd must be inside project {}", project.name).into());
        }
    }
    if spec.worktree_dir.is_none() && spec.branch.is_some() && spec.worktree != Some(false) {
        spec.worktree_dir = project.and_then(|p| p.worktree_dir.clone());
    }
    let cwd = match cwd {
        Some(cwd) => Some(cwd),
        None => project
            .map(|p| {
                PathBuf::from(&p.root)
                    .canonicalize()
                    .map_err(|e| format!("cannot resolve project directory: {e}"))
            })
            .transpose()?,
    };
    let source = cwd.unwrap_or_else(|| PathBuf::from("/"));
    if !source.is_dir() {
        return Err("agent cwd is not a directory".into());
    }
    prepare_agent(spec, source, None, data)
}

/// Other live agents whose folder is the one a reused worktree put this agent in.
/// Best effort: a screen that does not answer leaves the list empty rather than failing a
/// spawn whose pane already exists.
pub(super) fn occupants(senders: &ThreadSenders, spawned: &SpawnedAgent) -> Vec<PaneId> {
    let Some(cwd) = spawned.cwd.as_deref() else {
        return Vec::new();
    };
    let resolve = |path: &std::path::Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let cwd = resolve(cwd);
    let (reply, receiver) = std::sync::mpsc::channel();
    if senders
        .send_to_screen(ScreenInstruction::AgentControlReply(
            AgentRequest::List { project: None },
            reply,
        ))
        .is_err()
    {
        return Vec::new();
    }
    let Ok(listing) = receiver.recv_timeout(std::time::Duration::from_secs(5)) else {
        return Vec::new();
    };
    listing
        .agents
        .into_iter()
        .filter(|status| status.pane_id != spawned.pane_id)
        .filter(|status| {
            status
                .cwd
                .as_deref()
                .is_some_and(|other| resolve(std::path::Path::new(other)) == cwd)
        })
        .map(|status| status.pane_id)
        .collect()
}

/// What a spawn did that the caller did not literally ask for, in one line.
pub(super) fn spawn_notes(spawned: &SpawnedAgent) -> Option<String> {
    let mut notes = Vec::new();
    if let (Some(requested), Some(branch)) = (&spawned.requested_branch, &spawned.branch) {
        notes.push(if spawned.reused {
            format!("requested branch {requested}; the reused worktree holds {branch}")
        } else {
            format!("branch {requested} was taken; created {branch}")
        });
    }
    if !spawned.occupied_by.is_empty() {
        let panes: Vec<String> = spawned
            .occupied_by
            .iter()
            .map(ToString::to_string)
            .collect();
        notes.push(format!(
            "warning: this worktree is already used by {}; agents sharing a checkout can overwrite each other's changes",
            panes.join(", ")
        ));
    } else if spawned.reused {
        notes.push("reused an existing worktree".to_owned());
    }
    (!notes.is_empty()).then(|| notes.join("; "))
}

pub(super) fn register_spawned_agent(
    senders: &ThreadSenders,
    spawned: SpawnedAgent,
) -> AgentResponse {
    let mut response = AgentResponse::new(AgentOutcome::Ok);
    if let Err(error) = senders.send_to_screen(ScreenInstruction::AgentSpawned(spawned.clone())) {
        response.outcome = AgentOutcome::Failed;
        response.message = Some(format!(
            "agent pane was created but launch registration failed: {error:#}"
        ));
    }
    response.spawned = Some(spawned);
    response
}

#[cfg(test)]
#[path = "unit/agent_control.rs"]
mod tests;

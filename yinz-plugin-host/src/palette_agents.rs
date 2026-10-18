#[path = "palette_history.rs"]
mod history;
// Blocking native palette work. No Git operation or launch wait runs on screen.
use super::*;
use yinz_bus::command_palette::{
    AgentAction, AgentChoice, AgentProject, AgentReply, AgentRequest, Instruction,
};
use yinz_types::agent::control::AgentPlacement;
use yinz_types::data::PaneInfo;

pub(crate) fn handle(mut request: AgentRequest, senders: &ThreadSenders, data: &DataAccess) {
    // A newly opened shell can precede the first cwd broadcast. Query the captured
    // pane, never whichever pane happens to be focused when this worker runs.
    if request.source.cwd.is_none() && !matches!(request.action, AgentAction::History(_)) {
        let (tx, rx) = crossbeam::channel::bounded(1);
        if senders
            .send_to_pty(PtyInstruction::GetPaneCwd {
                pane_id: request.source.id,
                response_channel: tx,
            })
            .is_ok()
        {
            if let Ok(GetPaneCwdResponse::Ok(cwd)) = rx.recv_timeout(Duration::from_secs(5)) {
                request.source.cwd = Some(cwd.to_string_lossy().into_owned());
            }
        }
    }
    let reply = execute(&request, senders, data);
    senders
        .send_to_screen(ScreenInstruction::CommandPalette(Instruction::AgentReply {
            client: request.client,
            opening: request.opening,
            step: request.step,
            reply,
        }))
        .context("replying to native agent palette")
        .non_fatal();
}

fn still_wanted(request: &AgentRequest) -> Result<(), String> {
    if request.active.load(atomic::Ordering::Acquire) {
        Ok(())
    } else {
        Err("Agent action cancelled".into())
    }
}

fn current_source(request: &AgentRequest, senders: &ThreadSenders) -> Result<PaneInfo, String> {
    still_wanted(request)?;
    let (tx, rx) = crossbeam::channel::bounded(1);
    senders
        .send_to_screen(ScreenInstruction::GetPaneInfo {
            pane_id: request.source.id,
            response_channel: tx,
        })
        .map_err(|e| format!("cannot inspect source pane: {e}"))?;
    let pane = rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|e| format!("cannot inspect source pane: {e}"))?
        .ok_or("Source pane no longer exists")?;
    if pane.target != request.source.target {
        return Err("Source pane location changed; reopen the palette".into());
    }
    if matches!(
        request.action,
        AgentAction::Duplicate { .. } | AgentAction::Fork
    ) && (pane.agent.is_none()
        || pane.agent.as_ref().map(|a| (&a.kind, &a.session_id))
            != request
                .source
                .agent
                .as_ref()
                .map(|a| (&a.kind, &a.session_id)))
    {
        return Err("Source agent changed or is unavailable; reopen the palette".into());
    }
    Ok(pane)
}

fn project_root(
    project: &AgentProject,
    data: &DataAccess,
) -> Result<(PathBuf, Option<project_data::projects::Project>), String> {
    match project {
        AgentProject::Repository(root) => Ok((primary_worktree(root)?, None)),
        AgentProject::Registered(name) => {
            let registry = project_data::projects::from_rows(&agent_control::projects(data)?);
            let project = registry.get(name).ok_or("Project no longer exists")?;
            Ok((PathBuf::from(&project.root), Some(project.clone())))
        },
    }
}

fn execute(
    request: &AgentRequest,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> Result<AgentReply, String> {
    use AgentAction::*;
    let source = match &request.action {
        History(action) => return history::execute(request, action, senders, data),
        _ => current_source(request, senders)?,
    };
    let choice = |label: String, description: String, action| AgentChoice {
        label,
        description,
        action,
    };
    let choices = |title: String, choices| Ok(AgentReply::Choices { title, choices });
    match &request.action {
        History(_) => unreachable!("handled above"),
        NewProject { kind } => {
            if request.values.is_empty() {
                return Ok(AgentReply::Form {
                    title: format!("Spawn {kind} agent in new project"),
                    labels: vec![
                        "Project name".into(),
                        "Destination directory (absolute local path)".into(),
                    ],
                    submit: "Create and launch".into(),
                    action: request.action.clone(),
                });
            }
            let [name, destination] = request.values.as_slice() else {
                return Err("Project name and destination directory are required".into());
            };
            if name.trim().is_empty() || destination.trim().is_empty() {
                return Err("Project name and destination directory are required".into());
            }
            current_source(request, senders)?;
            Ok(AgentReply::Launched(agent_control::control_agent(
                yinz_types::agent::control::AgentRequest::SpawnNewProject {
                    name: name.clone(),
                    destination: destination.into(),
                    spec: AgentSpawnSpec {
                        kind: Some(kind.clone()),
                        ..Default::default()
                    },
                    placement: AgentPlacement::Automatic,
                },
                request.client,
                senders,
                data,
            )))
        },
        Start { kind, project } => {
            let registry = project_data::projects::from_rows(&agent_control::projects(data)?);
            let action = match project.as_ref().filter(|name| registry.contains_key(*name)) {
                Some(name) => Worktrees {
                    kind: kind.clone(),

                    project: AgentProject::Registered(name.clone()),
                },
                None => Projects { kind: kind.clone() },
            };
            execute(
                &AgentRequest {
                    action,
                    ..request.clone()
                },
                senders,
                data,
            )
        },
        Projects { kind } => {
            let registry = project_data::projects::from_rows(&agent_control::projects(data)?);
            let mut entries: Vec<_> = registry
                .values()
                .map(|p| {
                    choice(
                        p.name.clone(),
                        p.root.clone(),
                        Worktrees {
                            kind: kind.clone(),

                            project: AgentProject::Registered(p.name.clone()),
                        },
                    )
                })
                .collect();
            if !request.source.target.is_remote() {
                if let Some(cwd) = request.source.cwd.as_deref() {
                    if let Ok(root) = primary_worktree(Path::new(cwd)) {
                        entries.push(choice(
                            "Caller’s repository".into(),
                            root.display().to_string(),
                            Worktrees {
                                kind: kind.clone(),
                                project: AgentProject::Repository(root),
                            },
                        ));
                    }
                }
            }
            choices("Choose project".into(), entries)
        },
        Worktrees { kind, project } => {
            let (root, registered) = project_root(project, data)?;
            let name = registered
                .map(|p| p.name)
                .unwrap_or_else(|| root.display().to_string());
            let mut entries = vec![choice(
                "Create new worktree…".into(),
                String::new(),
                Checkout {
                    kind: kind.clone(),

                    project: project.clone(),
                    path: None,
                },
            )];
            entries.extend(
                worktrees(&root)?
                    .into_iter()
                    .filter(|w| w.path.is_dir())
                    .map(|w| {
                        choice(
                            w.branch.unwrap_or_else(|| "Detached HEAD".into()),
                            w.path.display().to_string(),
                            Checkout {
                                kind: kind.clone(),

                                project: project.clone(),
                                path: Some(w.path),
                            },
                        )
                    }),
            );
            entries.push(choice(
                "Change project…".into(),
                String::new(),
                Projects { kind: kind.clone() },
            ));
            choices(format!("{name} · Worktrees"), entries)
        },
        Checkout {
            kind,

            project,
            path,
        } => {
            let branch = if path.is_none() {
                let Some(value) = request.values.first() else {
                    return Ok(AgentReply::Form {
                        title: "Create new worktree".into(),
                        labels: vec!["Branch name".into()],
                        submit: "Launch".into(),
                        action: request.action.clone(),
                    });
                };
                if value.trim().is_empty() {
                    return Err("Branch name is required".into());
                }
                Some(value.clone())
            } else {
                None
            };
            let action = Launch {
                kind: kind.clone(),

                project: project.clone(),
                path: path.clone(),
                branch,
            };
            execute(
                &AgentRequest {
                    action,
                    values: vec![],
                    ..request.clone()
                },
                senders,
                data,
            )
        },
        Launch {
            kind,

            project,
            path,
            branch,
        } => {
            let (root, registered) = project_root(project, data)?;
            let cwd = if let Some(path) = path {
                let selected = worktree_at(&root, path)?
                    .ok_or("Selected worktree no longer belongs to this project")?;
                if !selected.path.is_dir() {
                    return Err("Selected worktree no longer exists".into());
                }
                selected.path
            } else {
                root
            };
            let spec = AgentSpawnSpec {
                kind: Some(kind.clone()),
                branch: branch.clone(),
                base: branch.as_ref().map(|_| {
                    let default = yinz_runtime::agent::launch::DEFAULT_BASE;
                    if branch_exists(&cwd, default) {
                        default.into()
                    } else {
                        // Worktree creation resolves HEAD in the primary checkout.
                        "HEAD".into()
                    }
                }),
                worktree_dir: registered
                    .as_ref()
                    .filter(|_| branch.is_some())
                    .and_then(|p| p.worktree_dir.clone()),
                // An explicitly selected checkout is reused exactly, including detached HEAD.
                allow_outside_worktree: Some(path.is_some()),
                ..Default::default()
            };
            launch(request, senders, spec, |spec| {
                prepare_agent(spec, cwd, None, data)
            })
        },
        Current { kind } => {
            let cwd = request
                .source
                .cwd
                .as_ref()
                .ok_or("Opening pane has no working directory")?;
            let target = request.source.target.clone();
            let spec = AgentSpawnSpec {
                kind: Some(kind.clone()),
                allow_outside_worktree: Some(true),
                ..Default::default()
            };
            launch(request, senders, spec, |_| {
                existing_command(kind, cwd, target)
            })
        },
        Fork => choices(
            "Fork focused agent · Choose agent".into(),
            yinz_runtime::agent::launch::offered()
                .into_iter()
                .map(|kind| choice(kind.clone(), String::new(), Duplicate { kind: Some(kind) }))
                .collect(),
        ),
        Duplicate { kind } => {
            let agent = source
                .agent
                .as_ref()
                .ok_or("Focused pane is not an agent")?;
            let kind = kind.as_ref().unwrap_or(&agent.kind);
            let cwd = agent
                .cwd
                .as_ref()
                .filter(|p| !p.is_empty())
                .or(source.cwd.as_ref())
                .ok_or("Source agent has no working directory")?;
            let spec = AgentSpawnSpec {
                kind: Some(kind.clone()),
                ..Default::default()
            };
            launch(request, senders, spec, |_| {
                existing_command(kind, cwd, source.target.clone())
            })
        },
    }
}

fn existing_command(
    kind: &str,
    cwd: &str,
    target: PaneTarget,
) -> Result<PreparedAgent, AgentSpawnError> {
    if !target.is_remote() && !Path::new(cwd).is_dir() {
        return Err("Agent directory no longer exists".into());
    }
    let mut command =
        yinz_runtime::agent::launch::command_in_context(kind, Some(cwd.into()), target)?;
    yinz_runtime::agent::pi::prepare_command(&mut command)?;
    Ok(PreparedAgent {
        command,
        cwd: cwd.into(),
        branch: None,
        requested_branch: None,
        reused: false,
    })
}

fn launch(
    request: &AgentRequest,
    senders: &ThreadSenders,
    spec: AgentSpawnSpec,
    prepare: impl FnOnce(&mut AgentSpawnSpec) -> Result<PreparedAgent, AgentSpawnError>,
) -> Result<AgentReply, String> {
    let still_valid = || current_source(request, senders).map(drop);
    still_valid()?;
    let prepare = |spec: &mut AgentSpawnSpec| {
        let mut prepared = prepare(spec)?;
        still_valid()
            .map_err(|error| AgentSpawnError::failed(error, Some(prepared.resources())))?;
        if prepared.command.path != Path::new("env") {
            prepared
                .command
                .args
                .insert(0, prepared.command.path.to_string_lossy().into_owned());
            prepared.command.path = "env".into();
        }
        prepared
            .command
            .args
            .insert(0, format!("{}=1", yinz_types::agent::AGENT_PANE_ENV));
        Ok(prepared)
    };
    let response = match spawn_agent_prepared(
        AgentPlacement::Automatic,
        spec,
        request.client,
        senders,
        prepare,
    ) {
        Ok(spawned) => agent_control::register_spawned_agent(senders, spawned),
        Err(error) => error.into_response(),
    };
    Ok(AgentReply::Launched(response))
}

#[cfg(test)]
#[path = "unit/palette_agents.rs"]
mod tests;

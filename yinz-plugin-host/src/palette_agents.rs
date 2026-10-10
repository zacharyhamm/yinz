#[path = "palette_history.rs"]
mod history;
// Blocking native palette work. No Git operation or launch wait runs on screen.
use super::*;
use yinz_bus::command_palette::{
    AgentAction, AgentChoice, AgentProject, AgentReply, AgentRequest, Instruction,
};
use yinz_types::agent::control::AgentPlacement;
use yinz_types::data::PaneInfo;

/// `session_cwd` is the directory the session was started in, where mother runs.
pub(crate) fn handle(
    mut request: AgentRequest,
    senders: &ThreadSenders,
    data: &DataAccess,
    session_cwd: &Path,
) {
    // A newly opened shell can precede the first cwd broadcast. Query the captured
    // pane, never whichever pane happens to be focused when this worker runs.
    if request.source.cwd.is_none()
        && !matches!(
            request.action,
            AgentAction::History(_) | AgentAction::Mother { .. }
        )
    {
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
    let reply = match &request.action {
        AgentAction::Mother { kind } => mother(&request, kind, session_cwd, senders),
        _ => execute(&request, senders, data),
    };
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
    if let AgentAction::History(action) = &request.action {
        return history::execute(request, action, senders, data);
    }
    let source = current_source(request, senders)?;
    let choice = |label: String, description: String, action| AgentChoice {
        label,
        description,
        action,
    };
    let choices = |title: String, choices| Ok(AgentReply::Choices { title, choices });
    match &request.action {
        History(_) => unreachable!("handled above"),
        Mother { .. } => unreachable!("handled by `handle`"),
        Start { kind, vm, project } => {
            let registry = project_data::projects::from_rows(&agent_control::projects(data)?);
            let action = match project.as_ref().filter(|name| registry.contains_key(*name)) {
                Some(name) => Worktrees {
                    kind: kind.clone(),
                    vm: *vm,
                    project: AgentProject::Registered(name.clone()),
                },
                None => Projects {
                    kind: kind.clone(),
                    vm: *vm,
                },
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
        Projects { kind, vm } => {
            let registry = project_data::projects::from_rows(&agent_control::projects(data)?);
            let mut entries: Vec<_> = registry
                .values()
                .map(|p| {
                    choice(
                        p.name.clone(),
                        p.root.clone(),
                        Worktrees {
                            kind: kind.clone(),
                            vm: *vm,
                            project: AgentProject::Registered(p.name.clone()),
                        },
                    )
                })
                .collect();
            if !vm && !request.source.target.is_remote() {
                if let Some(cwd) = request.source.cwd.as_deref() {
                    if let Ok(root) = primary_worktree(Path::new(cwd)) {
                        entries.push(choice(
                            "Caller’s repository".into(),
                            root.display().to_string(),
                            Worktrees {
                                kind: kind.clone(),
                                vm: false,
                                project: AgentProject::Repository(root),
                            },
                        ));
                    }
                }
            }
            choices("Choose project".into(), entries)
        },
        Worktrees { kind, vm, project } => {
            let (root, registered) = project_root(project, data)?;
            let name = registered
                .map(|p| p.name)
                .unwrap_or_else(|| root.display().to_string());
            let mut entries = vec![choice(
                "Create new worktree…".into(),
                String::new(),
                Checkout {
                    kind: kind.clone(),
                    vm: *vm,
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
                                vm: *vm,
                                project: project.clone(),
                                path: Some(w.path),
                            },
                        )
                    }),
            );
            entries.push(choice(
                "Change project…".into(),
                String::new(),
                Projects {
                    kind: kind.clone(),
                    vm: *vm,
                },
            ));
            choices(format!("{name} · Worktrees"), entries)
        },
        Checkout {
            kind,
            vm,
            project,
            path,
        } => {
            let branch = if path.is_none() {
                let Some(value) = request.value.as_ref() else {
                    return Ok(AgentReply::Form {
                        title: "Create new worktree".into(),
                        label: "Branch name".into(),
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
            let (_, registered) = project_root(project, data)?;
            let image = registered.and_then(|p| p.firecracker_image);
            let action = Launch {
                kind: kind.clone(),
                vm: *vm,
                project: project.clone(),
                path: path.clone(),
                branch,
                image,
            };
            execute(
                &AgentRequest {
                    action,
                    value: None,
                    ..request.clone()
                },
                senders,
                data,
            )
        },
        Launch {
            kind,
            vm,
            project,
            path,
            branch,
            image,
        } => {
            let (root, registered) = project_root(project, data)?;
            let image = image
                .clone()
                .or_else(|| request.value.clone().filter(|v| !v.trim().is_empty()));
            if *vm && image.is_none() && !default_vm_image_available() {
                if request.value.is_some() {
                    return Err("Image name is required".into());
                }
                return Ok(AgentReply::Form {
                    title: "Firecracker image".into(),
                    label: "Image name".into(),
                    action: request.action.clone(),
                });
            }
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
                firecracker: vm.then(|| "palette-agent".into()),
                firecracker_image: image.filter(|_| *vm),
                // An explicitly selected checkout is reused exactly, including detached HEAD.
                allow_outside_worktree: Some(path.is_some()),
                ..Default::default()
            };
            launch(request, senders, spec, TILED, |spec| {
                prepare_agent(spec, cwd, None, *vm, registered.is_some(), false, data)
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
            launch(request, senders, spec, TILED, |_| {
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
            launch(request, senders, spec, TILED, |_| {
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
    if !command.target.is_remote() {
        yinz_runtime::agent::pi::prepare_command(&mut command)?;
    } else {
        yinz_runtime::agent::pi::strip_extension(&mut command.args);
    }
    Ok(PreparedAgent {
        command,
        cwd: cwd.into(),
        branch: None,
        requested_branch: None,
        reused: false,
    })
}

const TILED: NewPanePlacement = NewPanePlacement::Tiled {
    direction: None,
    borderless: None,
};

/// Launch the session's mother agent: a fresh conversation in the directory the session
/// started in, on the session surface instead of in a tab's tiled layout.
fn mother(
    request: &AgentRequest,
    kind: &str,
    session_cwd: &Path,
    senders: &ThreadSenders,
) -> Result<AgentReply, String> {
    let cwd = session_cwd.to_string_lossy();
    let spec = AgentSpawnSpec {
        kind: Some(kind.to_owned()),
        allow_outside_worktree: Some(true),
        ..Default::default()
    };
    let surface =
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(FloatingPaneCoordinates::new(
            Some("10%".into()),
            Some("10%".into()),
            Some("80%".into()),
            Some("80%".into()),
            None,
            None,
        )));
    let reply = launch(request, senders, spec, surface, |_| {
        existing_command(kind, &cwd, PaneTarget::Local)
    })?;
    if let AgentReply::Launched(AgentResponse {
        outcome: AgentOutcome::Ok,
        spawned: Some(spawned),
        ..
    }) = &reply
    {
        // The screen records mother from this, not from the palette reply, which it drops
        // when the palette was dismissed while the agent started.
        senders
            .send_to_screen(ScreenInstruction::MotherSpawned(spawned.pane_id))
            .context("registering mother")
            .non_fatal();
    }
    Ok(reply)
}

fn launch(
    request: &AgentRequest,
    senders: &ThreadSenders,
    spec: AgentSpawnSpec,
    surface: NewPanePlacement,
    prepare: impl FnOnce(&mut AgentSpawnSpec) -> Result<PreparedAgent, AgentSpawnError>,
) -> Result<AgentReply, String> {
    // Mother belongs to the session, so the pane the palette opened from may come and go.
    let still_valid = || match request.action {
        AgentAction::Mother { .. } => still_wanted(request),
        _ => current_source(request, senders).map(drop),
    };
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
        surface,
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

// The provider's default is a local rootfs, independent of project image names.
fn default_vm_image_available() -> bool {
    let config = std::env::var_os("YINZ_FIRECRACKER_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| yinz_runtime::home::xdg_config_dir().join("firecracker.json"));
    fs::read(config)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|config| {
            config
                .get("rootfs")
                .and_then(|p| p.as_str())
                .map(PathBuf::from)
        })
        .is_some_and(|path| path.is_file())
}

#[cfg(test)]
#[path = "unit/palette_agents.rs"]
mod tests;

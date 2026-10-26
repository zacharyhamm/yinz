//! Agent launch preparation, worktree ownership, and placement completion.

use super::filesystem::resolve_fs_ancestors;
use super::pane_creation::{spawn_command_pane_near_plugin, translate_plugin_path};
use super::write_response;
use crate::plugin_map::PluginEnv;
use prost::Message;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use yinz_bus::global_async_runtime::get_tokio_runtime;
use yinz_bus::pty::{ClientTabIndexOrPaneId, PtyInstruction};
use yinz_bus::route::NotificationEnd;
use yinz_bus::screen::ScreenInstruction;
use yinz_bus::thread_bus::ThreadSenders;
use yinz_plugin_api::plugin_command::ProtobufSpawnAgentResponse;
use yinz_runtime::errors::prelude::*;
use yinz_storage::data_access::DataAccess;
use yinz_types::agent::control::{AgentOutcome, AgentResponse};
use yinz_types::agent::{AgentLaunchResources, AgentSpawnSpec, SpawnedAgent};
use yinz_types::command::{CommandToRun, GitCheckout, PaneTarget};
use yinz_types::data::{
    DataError, FetchRequest, NewPanePlacement, SpawnAgentResponse, AGENT_PANE_CONTEXT,
};
use yinz_types::input::command::{RunCommand, RunCommandAction, TerminalAction};
use yinz_types::input::layout::Run;

/// Start a coding agent near this plugin: the launch planner every agent launcher uses, the
/// checkout the spec asks for, the pane, and the agent record - one road for agent's
/// `agent_spawn`, the `new-agent` modal and a Scheme plugin's agent spawn command.
pub(super) fn spawn_agent(
    env: &mut PluginEnv,
    spec: AgentSpawnSpec,
    source_directory: Option<PathBuf>,
    cwd: Option<PathBuf>,
    name: Option<String>,
    mut context: BTreeMap<String, String>,
    start_suppressed: bool,
) {
    let result: SpawnAgentResponse = (|| {
        let host_paths = env.plugin_config.is_builtin()
            && context
                .get("agent_agent_host_cwd")
                .is_some_and(|value| value == "true");
        let source = source_directory
            .map(|path| {
                if host_paths && path.is_absolute() {
                    path
                } else {
                    translate_plugin_path(env, path)
                }
            })
            .unwrap_or_else(|| env.plugin_cwd.clone());
        let cwd = cwd.map(|path| {
            if host_paths && path.is_absolute() {
                path
            } else {
                translate_plugin_path(env, path)
            }
        });
        let prepared = prepare_agent(&spec, source, cwd, &env.data_access)
            .map_err(|error| error.to_string())?;
        let resources = prepared.resources();
        let kind = yinz_types::agent::agent_kind_of(&prepared.command.path, &prepared.command.args)
            .unwrap_or_else(|| "true".to_owned());
        context.insert(AGENT_PANE_CONTEXT.to_owned(), kind);
        spawn_command_pane_near_plugin(env, prepared.command, context, name, true, start_suppressed)
            .map(|pane_id| SpawnedAgent {
                pane_id,
                branch: resources.branch,
                cwd: resources.cwd,
                ..Default::default()
            })
    })();
    let response = ProtobufSpawnAgentResponse::from(result);
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write spawn_agent response".to_string())
        .non_fatal();
}

#[derive(Debug)]
pub(super) struct PreparedAgent {
    pub(super) command: CommandToRun,
    pub(super) branch: Option<String>,
    pub(super) cwd: PathBuf,
    /// The branch asked for, when `branch` is a different one.
    pub(super) requested_branch: Option<String>,
    pub(super) reused: bool,
}

impl PreparedAgent {
    pub(super) fn resources(&self) -> AgentLaunchResources {
        AgentLaunchResources {
            tab_id: None,
            created_tab_id: None,
            cwd: Some(self.cwd.clone()),
            branch: self.branch.clone(),
            requested_branch: self.requested_branch.clone(),
            reused: self.reused,
        }
    }
}

#[derive(Debug)]
pub(super) struct AgentSpawnError(Box<AgentResponse>);

impl AgentSpawnError {
    pub(super) fn failed(
        message: impl Into<String>,
        resources: Option<AgentLaunchResources>,
    ) -> Self {
        let mut response = AgentResponse::new(AgentOutcome::Failed);
        response.message = Some(format!(
            "{}; inspect the launch resources and agent list before retrying",
            message.into()
        ));
        response.launch_resources = resources;
        Self(Box::new(response))
    }

    pub(super) fn into_response(self) -> AgentResponse {
        *self.0
    }
}

impl From<String> for AgentSpawnError {
    fn from(message: String) -> Self {
        let mut response = AgentResponse::new(AgentOutcome::Rejected);
        response.message = Some(message);
        Self(Box::new(response))
    }
}

impl From<&str> for AgentSpawnError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl std::fmt::Display for AgentSpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.message.as_deref().unwrap_or("agent launch failed"))
    }
}

/// Shared checkout preparation for plugin and direct CLI launches. Paths are already host paths.
pub(super) fn prepare_agent(
    spec: &AgentSpawnSpec,
    source: PathBuf,
    cwd: Option<PathBuf>,
    data_access: &DataAccess,
) -> Result<PreparedAgent, AgentSpawnError> {
    let (mut command, mut checkout) = yinz_runtime::agent::launch::plan(spec)?;
    let bubblewrap = if spec.bubblewrap {
        if !command.target.is_local() {
            return Err("Bubblewrap cannot be combined with SSH".into());
        }
        let validated_cwd = match &checkout {
            GitCheckout::Current => cwd.as_deref().unwrap_or(&source),
            _ => &source,
        }
        .canonicalize()
        .map_err(|error| format!("resolve Bubblewrap launch directory: {error}"))?;
        let environment =
            yinz_config::project_config::bubblewrap::Environment::for_project(&validated_cwd)
                .map_err(|error| format!("{error:#}"))?;
        environment
            .preflight()
            .map_err(|error| format!("{error:#}"))?;
        environment
            .validate_spawn_env(&spec.env)
            .map_err(|error| format!("{error:#}"))?;
        Some((environment, validated_cwd))
    } else {
        None
    };

    apply_project_worktree_dir(&mut checkout, spec.worktree_dir.as_deref(), &source, || {
        data_access.plugin_data().fetch(
            project_data::store::SHARED,
            &FetchRequest::entity("project").limit(project_data::store::LIST_FETCH_LIMIT),
        )
    })?;
    let requested_branch = match &checkout {
        GitCheckout::NewBranch { branch, .. } | GitCheckout::NewWorktree { branch, .. } => {
            Some(branch.clone())
        },
        GitCheckout::Current => None,
    };
    // An agent loose in the checkout the user is working in - or in a folder that is no
    // repository at all - is a thing to ask for, not a thing to fall into by leaving
    // `branch` out. The resolved folder is what is checked, not the spec: a branch held
    // by another worktree sends the agent there, and that is a worktree.
    let guard = if spec.allow_outside_worktree.unwrap_or(false) {
        WorktreeGuard::Waived
    } else {
        WorktreeGuard::Required
    };
    // Resolve reporting assets before Git changes, so an unwritable data directory cannot
    // strand a fresh checkout. A reused opening prompt only changes the final command text.
    yinz_runtime::agent::pi::prepare_command(&mut command)?;
    let mut attempted = None;
    let prepared = agent_folder_tracked(
        checkout,
        cwd,
        source,
        spec.reuse.unwrap_or(false),
        guard,
        &mut attempted,
        bubblewrap
            .as_ref()
            .map(|(environment, _)| environment.root()),
    )
    .map_err(|error| match attempted {
        Some(resources) => AgentSpawnError::failed(error, Some(resources)),
        None => error.into(),
    })?;
    let resources = AgentLaunchResources {
        tab_id: None,
        created_tab_id: None,
        cwd: Some(prepared.path.clone()),
        branch: prepared.branch.clone(),
        requested_branch: requested_branch
            .clone()
            .filter(|requested| prepared.branch.as_ref() != Some(requested)),
        reused: prepared.reused_worktree,
    };
    if prepared.reused_worktree && spec.reuse_prompt.is_some() {
        command = prepared
            .agent_command(spec)
            .map_err(|error| AgentSpawnError::failed(error, Some(resources.clone())))?;
        yinz_runtime::agent::pi::prepare_command(&mut command)
            .map_err(|error| AgentSpawnError::failed(error, Some(resources.clone())))?;
    }
    let reused = prepared.reused_worktree;
    let branch = prepared.branch.or_else(|| {
        worktree_at(&prepared.path, &prepared.path)
            .ok()
            .flatten()
            .and_then(|worktree| worktree.branch)
    });
    let folder = prepared.path;
    command.cwd = Some(folder.clone());
    if let Some((environment, validated_cwd)) = bubblewrap {
        // Only a different destination needs another check here; launch and exec revalidate it.
        if folder.canonicalize().ok().as_ref() != Some(&validated_cwd) {
            environment
                .validate_cwd(&folder)
                .map_err(|error| AgentSpawnError::failed(format!("{error:#}"), Some(resources)))?;
        }
        command.target = PaneTarget::Bubblewrap {
            root: environment.root().to_path_buf(),
        };
    }
    let requested_branch = requested_branch.filter(|requested| branch.as_ref() != Some(requested));
    Ok(PreparedAgent {
        command,
        branch,
        cwd: folder,
        requested_branch,
        reused,
    })
}

/// A CLI-spawned agent pane's title, such as `claude (haiku)`.
///
/// Without one the title is the whole command line, which for claude carries kilobytes of
/// hook wiring. `None` for a raw command, which keeps that default.
fn agent_pane_title(spec: &AgentSpawnSpec) -> Option<String> {
    let nonblank = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let kind = yinz_runtime::agent::launch::kind_for_spec(spec)?;
    Some(match nonblank(&spec.model) {
        Some(model) => format!("{kind} ({model})"),
        None => kind,
    })
}

/// Where an agent's pane goes unless the launch says otherwise.
pub(super) const TILED: NewPanePlacement = NewPanePlacement::Tiled {
    direction: None,
    borderless: None,
};

/// `placement` picks the tab. A tiled pane may
/// be redirected to a roomier tab (`Screen::agent_pane_target`).
pub(super) fn spawn_agent_prepared(
    placement: yinz_types::agent::control::AgentPlacement,
    mut spec: AgentSpawnSpec,
    client_id: crate::ClientId,
    senders: &ThreadSenders,
    prepare: impl FnOnce(&mut AgentSpawnSpec) -> Result<PreparedAgent, AgentSpawnError>,
) -> Result<SpawnedAgent, AgentSpawnError> {
    let surface = TILED;
    let started = Instant::now();
    if spec.prompt.is_some() || spec.reuse_prompt.is_some() {
        return Err(
            "spawn does not accept prompt or reuse_prompt; spawn, wait, then prompt".into(),
        );
    }
    use yinz_types::agent::control::AgentPlacement;
    placement.validate()?;
    let start_suppressed = matches!(placement, AgentPlacement::Suppressed(_));
    let mut destination = match &placement {
        AgentPlacement::Tab(_) | AgentPlacement::Suppressed(_) => Some(
            resolve_agent_tab(senders, placement.clone(), None, client_id).map_err(|error| {
                let mut failure = AgentSpawnError::from(error);
                failure.0.launch_resources = Some(AgentLaunchResources {
                    tab_id: match placement {
                        AgentPlacement::Tab(id) | AgentPlacement::Suppressed(Some(id)) => Some(id),
                        _ => None,
                    },
                    ..Default::default()
                });
                failure
            })?,
        ),
        _ => None,
    };
    let prepared = prepare(&mut spec).map_err(|mut failure| {
        if let Some(destination) = &destination {
            failure
                .0
                .launch_resources
                .get_or_insert_with(Default::default)
                .tab_id = Some(destination.tab_id);
        }
        failure
    })?;
    let mut resources = prepared.resources();
    resources.tab_id = destination.as_ref().map(|target| target.tab_id);
    let command = prepared.command;
    let title = agent_pane_title(&spec);
    let action = RunCommandAction {
        command: command.path,
        args: command.args,
        cwd: command.cwd,
        hold_on_close: true,
        target: command.target,
        remote_default_shell: command.remote_default_shell,
        ..Default::default()
    };
    let action: RunCommand = action.into();
    if matches!(placement, AgentPlacement::NewTab(_)) {
        destination = Some(
            resolve_agent_tab(
                senders,
                placement,
                Some(Run::Command(action.clone())),
                client_id,
            )
            .map_err(|error| AgentSpawnError::failed(error, Some(resources.clone())))?,
        );
        resources.tab_id = destination.as_ref().map(|target| target.tab_id);
        resources.created_tab_id = resources.tab_id;
    }
    let (completion_tx, completion_rx) = oneshot::channel();
    let mut completion = NotificationEnd::new(completion_tx);
    completion.wait_for_agent_start = true;
    completion.agent_tab = destination.clone();
    completion.created_tab_id = resources.created_tab_id;
    let target = destination.as_ref().map_or(
        ClientTabIndexOrPaneId::ClientIdNoFocus(client_id),
        |destination| ClientTabIndexOrPaneId::TabIndexNoFocus(destination.tab_id),
    );
    senders
        .send_to_pty(PtyInstruction::SpawnTerminal(
            Some(TerminalAction::RunCommand(action)),
            title,
            surface,
            start_suppressed,
            target,
            Some(completion),
            false,
        ))
        .map_err(|error| {
            AgentSpawnError::failed(
                format!(
                    "failed to request agent pane: {error:#}{}",
                    cleanup_agent_tab(senders, destination.as_ref(), &mut resources)
                ),
                Some(resources.clone()),
            )
        })?;
    let result = wait_for_agent_placement(
        completion_rx,
        &resources,
        Duration::from_secs(120).saturating_sub(started.elapsed()),
    )?;
    destination = destination.or(result.agent_tab);
    resources.tab_id = resources
        .tab_id
        .or(destination.as_ref().map(|target| target.tab_id));
    resources.created_tab_id = resources.created_tab_id.or(result.created_tab_id);
    let definitive_failure = result.error_message.is_some();
    let error = result.error_message.or_else(|| {
        if result.affected_pane_id.is_none() {
            Some("agent pane creation outcome is unknown".into())
        } else if result.affected_tab_id.is_none() {
            Some("agent destination outcome is unknown".into())
        } else {
            None
        }
    });
    if let Some(error) = error {
        let cleanup = if definitive_failure {
            cleanup_agent_tab(senders, destination.as_ref(), &mut resources)
        } else {
            String::new()
        };
        let mut failure =
            AgentSpawnError::failed(format!("{error}{cleanup}"), Some(resources.clone()));
        if let Some(pane_id) = result.affected_pane_id {
            failure.0.spawned = Some(SpawnedAgent {
                pane_id,
                tab_id: result.affected_tab_id,
                created_tab_id: resources.created_tab_id,
                branch: resources.branch.clone(),
                cwd: resources.cwd.clone(),
                requested_branch: resources.requested_branch.clone(),
                reused: resources.reused,
                occupied_by: Vec::new(),
            });
            if resources.tab_id.is_none() && resources.created_tab_id.is_none() {
                failure.0.launch_resources = None;
            }
        }
        return Err(failure);
    }
    let pane_id = result.affected_pane_id.ok_or_else(|| {
        AgentSpawnError::failed(
            "agent pane creation outcome is unknown",
            Some(resources.clone()),
        )
    })?;
    let tab_id = result.affected_tab_id.ok_or_else(|| {
        AgentSpawnError::failed(
            "agent destination outcome is unknown",
            Some(resources.clone()),
        )
    })?;
    Ok(SpawnedAgent {
        tab_id: Some(tab_id),
        created_tab_id: resources.created_tab_id,
        pane_id,
        branch: resources.branch,
        cwd: resources.cwd,
        requested_branch: resources.requested_branch,
        reused: resources.reused,
        occupied_by: Vec::new(),
    })
}

/// A timeout ends the caller's wait, not a process or layout that may still complete.
pub(super) fn wait_for_agent_placement(
    receiver: oneshot::Receiver<yinz_bus::route::ActionCompletionResult>,
    resources: &AgentLaunchResources,
    timeout: Duration,
) -> Result<yinz_bus::route::ActionCompletionResult, AgentSpawnError> {
    match get_tokio_runtime().block_on(async { tokio::time::timeout(timeout, receiver).await }) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) => Err(AgentSpawnError::failed(
            "agent placement outcome is unknown",
            Some(resources.clone()),
        )),
        Err(_) => {
            let mut failure = AgentSpawnError::failed(
                "agent launch timed out; outcome is unknown and a late placement may still complete",
                Some(resources.clone()),
            );
            failure.0.outcome = AgentOutcome::TimedOut;
            Err(failure)
        },
    }
}

/// Resolve existing destinations before checkout preparation; create new ones afterwards.
fn resolve_agent_tab(
    senders: &ThreadSenders,
    placement: yinz_types::agent::control::AgentPlacement,
    invoked_with: Option<Run>,
    client_id: crate::ClientId,
) -> Result<yinz_bus::route::AgentTabPlacement, String> {
    let (reply, receiver) = std::sync::mpsc::channel();
    senders
        .send_to_screen(ScreenInstruction::ResolveAgentTab {
            placement,
            invoked_with,
            client_id,
            reply,
        })
        .map_err(|error| format!("cannot request agent destination: {error:#}"))?;
    receiver.recv().map_err(|error| {
        format!("agent destination outcome unknown: {error}; inspect tabs before retrying")
    })?
}

fn cleanup_agent_tab(
    senders: &ThreadSenders,
    destination: Option<&yinz_bus::route::AgentTabPlacement>,
    resources: &mut AgentLaunchResources,
) -> String {
    let Some(destination) = destination.filter(|target| target.created) else {
        return if resources.created_tab_id.is_some() {
            "; created tab retained; inspect it before retrying".into()
        } else {
            String::new()
        };
    };
    let (reply, receiver) = std::sync::mpsc::channel();
    let cleaned = senders
        .send_to_screen(ScreenInstruction::CleanupAgentTab(
            destination.clone(),
            reply,
        ))
        .is_ok()
        && receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or(false);
    if cleaned {
        resources.created_tab_id = None;
        "; launch-created tab is closed".into()
    } else {
        "; launch-created tab retained or cleanup uncertain; inspect it before retrying".into()
    }
}

/// Read saved defaults only for a new worktree with no launch override. Fetching all
/// columns also works before the nullable setting has been added to an old registry.
pub(super) fn apply_project_worktree_dir(
    checkout: &mut GitCheckout,
    explicit: Option<&str>,
    source: &std::path::Path,
    fetch: impl FnOnce() -> Result<yinz_types::data::DataRows, DataError>,
) -> Result<(), String> {
    let GitCheckout::NewWorktree { branch, path, .. } = checkout else {
        return Ok(());
    };
    if explicit.is_some() {
        return Ok(());
    }
    let rows = match fetch() {
        Ok(rows) => rows,
        Err(DataError::UnknownEntity(_)) => return Ok(()),
        Err(error) => {
            return Err(format!(
                "could not read project worktree directory: {error}"
            ))
        },
    };
    let mut projects = project_data::projects::from_rows(&rows);
    // Match filesystem identities before lexical containment, including /var aliases on macOS.
    for project in projects.values_mut() {
        if let Ok(root) = std::fs::canonicalize(&project.root) {
            project.root = root.to_string_lossy().into_owned();
        }
    }
    let source = source.canonicalize().unwrap_or_else(|_| source.to_owned());
    let location = project_data::location::Location::local(source.to_string_lossy());
    let project = match project_data::projects::for_cwd(&projects, &location) {
        Some(project) => Some(project),
        None if projects.is_empty() => None,
        None => {
            let checkouts = worktrees(&source)?;
            let (primary, linked) = checkouts
                .split_first()
                .ok_or_else(|| "git could not find the primary worktree".to_owned())?;
            project_data::projects::for_cwd(
                &projects,
                &project_data::location::Location::local(primary.path.to_string_lossy()),
            )
            .or_else(|| {
                let mut matches = projects.values().filter(|project| {
                    linked.iter().any(|checkout| {
                        same_path(std::path::Path::new(&project.root), &checkout.path)
                    })
                });
                let project = matches.next()?;
                // Several linked roots can be registered with different defaults.
                matches.next().is_none().then_some(project)
            })
        },
    };
    if let Some(dir) = project.and_then(|project| project.worktree_dir.as_deref()) {
        if dir.contains('\0') {
            return Err("worktree_dir must not contain NUL bytes".to_owned());
        }
        *path = yinz_runtime::agent::launch::worktree_path_in(Some(dir), branch).into();
    }
    Ok(())
}

/// Where a spawned agent's pane runs. A branch means git work in `source`, the repository, and
/// the pane lands in the checkout that makes. Without one there is no git work at all: the
/// pane runs in `cwd` when the caller named one, and otherwise in `source` as it stands -
/// agent sends the focused pane's folder, a Scheme plugin's default is its own - never the
/// repository root, which `command=zsh` in a subfolder of a checkout does not want and which
/// costs a `git rev-parse` to find.
#[cfg(test)]
pub(super) fn agent_folder(
    checkout: GitCheckout,
    cwd: Option<PathBuf>,
    source: PathBuf,
    reuse: bool,
    guard: WorktreeGuard,
) -> Result<PreparedCheckout, String> {
    agent_folder_tracked(checkout, cwd, source, reuse, guard, &mut None, None)
}

fn validate_checkout_root(path: &Path, root: Option<&Path>) -> Result<(), String> {
    if let Some(root) = root {
        let resolved = resolve_fs_ancestors(path).map_err(|error| format!("{error:#}"))?;
        if !resolved.starts_with(root) {
            return Err(format!(
                "Bubblewrap checkout must be inside {}",
                root.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn agent_folder_tracked(
    checkout: GitCheckout,
    cwd: Option<PathBuf>,
    source: PathBuf,
    reuse: bool,
    guard: WorktreeGuard,
    attempted: &mut Option<AgentLaunchResources>,
    bubblewrap_root: Option<&Path>,
) -> Result<PreparedCheckout, String> {
    match (checkout, cwd) {
        (GitCheckout::Current, Some(cwd)) => {
            guard.check(&cwd)?;
            Ok(PreparedCheckout {
                reused_worktree: false,
                path: cwd,
                branch: None,
            })
        },
        (GitCheckout::Current, None) => {
            guard.check(&source)?;
            Ok(PreparedCheckout {
                reused_worktree: false,
                path: source,
                branch: None,
            })
        },
        (checkout, _) => prepare_git_checkout_in_tracked(
            source,
            checkout,
            reuse,
            guard,
            attempted,
            bubblewrap_root,
        ),
    }
}

/// Whether the folder an agent is about to be left in has to be a *linked* worktree.
///
/// Asked at each point where a destination is settled and **before any git work is done**,
/// rather than of the folder a finished checkout answers with: a refusal that has already
/// switched the user's checkout onto a new branch has left them worse off than the spawn
/// they asked for.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum WorktreeGuard {
    Required,
    Waived,
}

impl WorktreeGuard {
    fn check(self, path: &std::path::Path) -> Result<(), String> {
        if self == WorktreeGuard::Waived || linked_worktree(path)? {
            return Ok(());
        }
        Err(format!(
            "{} is not a linked git worktree; give a branch to cut one, or set \
             allow_outside_worktree",
            path.display()
        ))
    }
}

pub(super) fn prepare_git_checkout(
    env: &PluginEnv,
    source_directory: PathBuf,
    checkout: GitCheckout,
) -> Result<PathBuf, String> {
    let source_directory = translate_plugin_path(env, source_directory);
    // Reuse, because that is what this call has always meant and what a plugin opening a
    // command pane in a checkout asks for: the branch it names is the work it is pointing
    // at. Only `spawn_agent` asks for the other thing, and it says so.
    // No guard: this command has never had one. It puts a *plugin's* command pane in a
    // checkout rather than leaving an agent loose in one.
    prepare_git_checkout_in(source_directory, checkout, true, WorktreeGuard::Waived)
        .map(|prepared| prepared.path)
}

/// A checkout the host has made or found: the folder to run in, and the branch it holds when
/// there is one. The branch is answered rather than assumed because it is not always the one
/// asked for - a taken name is cut afresh with a suffix.
#[derive(Debug)]
pub(super) struct PreparedCheckout {
    pub(super) path: PathBuf,
    pub(super) branch: Option<String>,
    pub(super) reused_worktree: bool,
}

impl PreparedCheckout {
    pub(super) fn agent_command(&self, spec: &AgentSpawnSpec) -> Result<CommandToRun, String> {
        let mut spec = spec.clone();
        if self.reused_worktree
            && let Some(prompt) = spec.reuse_prompt.take()
        {
            spec.prompt = Some(prompt);
        }
        yinz_runtime::agent::launch::plan(&spec).map(|(command, _)| command)
    }
}

/// Put the repository into the state the request describes, and answer with the
/// folder the command should run in.
///
/// With `reuse`, "new" in [`GitCheckout::NewBranch`] and [`GitCheckout::NewWorktree`]
/// means *the one asked for*, created if it is not there yet: a branch or a worktree
/// that already exists is the work the caller is pointing at, and the agent belongs in
/// it. Only a path that exists without being a worktree of this repository still fails,
/// because that is a genuine clash rather than something to reuse.
///
/// Without it, a name already taken is a collision and the next free one is cut instead -
/// `<branch>-2`, `<branch>-3`, and the worktree renamed with it, so the two never come
/// apart. Two agents asked for the same branch by two different people is the case this
/// is for: joining them silently is how one of them loses work.
pub(super) fn prepare_git_checkout_in(
    source_directory: PathBuf,
    checkout: GitCheckout,
    reuse: bool,
    guard: WorktreeGuard,
) -> Result<PreparedCheckout, String> {
    prepare_git_checkout_in_tracked(source_directory, checkout, reuse, guard, &mut None, None)
}

pub(super) fn prepare_git_checkout_in_tracked(
    source_directory: PathBuf,
    checkout: GitCheckout,
    reuse: bool,
    guard: WorktreeGuard,
    attempted: &mut Option<AgentLaunchResources>,
    bubblewrap_root: Option<&Path>,
) -> Result<PreparedCheckout, String> {
    let mut before_git_write = |path: &Path, branch: &str, requested: &str| {
        validate_checkout_root(path, bubblewrap_root)?;
        *attempted = Some(AgentLaunchResources {
            tab_id: None,
            created_tab_id: None,
            cwd: Some(path.to_owned()),
            branch: Some(branch.to_owned()),
            requested_branch: (branch != requested).then(|| requested.to_owned()),
            reused: false,
        });
        Ok::<(), String>(())
    };
    let root_output = process::Command::new("git")
        .arg("-C")
        .arg(&source_directory)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !root_output.status.success() {
        return Err(git_error("find the repository root", &root_output.stderr));
    }
    let root = PathBuf::from(String::from_utf8_lossy(&root_output.stdout).trim());
    match checkout {
        GitCheckout::Current => {
            guard.check(&root)?;
            Ok(PreparedCheckout {
                reused_worktree: false,
                path: root,
                branch: None,
            })
        },
        GitCheckout::NewBranch { branch, base } => {
            validate_git_names(&root, &branch, &base)?;
            if reuse {
                // A branch checked out somewhere else cannot be switched to here -
                // git allows one checkout of a branch at a time - so the agent goes
                // to the worktree already holding it rather than nowhere at all.
                if let Some(worktree) = worktree_holding(&root, &branch)? {
                    guard.check(&worktree)?;
                    return Ok(PreparedCheckout {
                        reused_worktree: true,
                        path: worktree,
                        branch: Some(branch),
                    });
                }
            }
            // Every route left lands in `root` - this checkout moves to the branch - so
            // the guard is settled here, before the switch that would otherwise leave a
            // refused request with the user's checkout already moved.
            guard.check(&root)?;
            if reuse && branch_exists(&root, &branch) {
                before_git_write(&root, &branch, &branch)?;
                run_git(&root, &["switch", &branch], "switch to the branch")?;
                return Ok(PreparedCheckout {
                    reused_worktree: false,
                    path: root,
                    branch: Some(branch),
                });
            }
            let requested = branch.clone();
            let branch = if reuse {
                branch
            } else {
                free_branch(&root, &branch)?
            };
            before_git_write(&root, &branch, &requested)?;
            run_git(
                &root,
                &["switch", "-c", &branch, &base],
                "create and switch to the branch",
            )?;
            Ok(PreparedCheckout {
                reused_worktree: false,
                path: root,
                branch: Some(branch),
            })
        },
        GitCheckout::NewWorktree { branch, base, path } => {
            // Anchored to the primary checkout, not to the caller's. A relative path
            // joined onto the caller's root nests a worktree inside a worktree
            // whenever the agent asking for one is already sitting in a worktree,
            // and a nested worktree is stranded the moment its parent is removed.
            let root = primary_worktree(&root)?;
            validate_git_names(&root, &branch, &base)?;
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            if reuse {
                // The path first, because it is the more specific of the two ways a
                // request can already be satisfied: a worktree sitting there is the
                // folder the caller named, whatever branch it happens to hold, and
                // nothing about it is changed. That branch is what is answered - the
                // one asked for was not used - and a detached checkout holds none.
                if let Some(existing) = worktree_at(&root, &path)? {
                    guard.check(&existing.path)?;
                    return Ok(PreparedCheckout {
                        reused_worktree: true,
                        path: existing.path,
                        branch: existing.branch,
                    });
                }
                if let Some(existing) = worktree_holding(&root, &branch)? {
                    guard.check(&existing)?;
                    return Ok(PreparedCheckout {
                        reused_worktree: true,
                        path: existing,
                        branch: Some(branch),
                    });
                }
                let path_string = path.to_string_lossy().into_owned();
                // An existing branch already has a history, so it is checked out as
                // it stands and the base has nothing to say about it.
                // What is cut below needs no check and could not have one: `git worktree
                // add` always makes a linked worktree, and `linked_worktree` of a folder
                // that does not exist yet answers false.
                let args: Vec<&str> = if branch_exists(&root, &branch) {
                    vec!["worktree", "add", "--", &path_string, &branch]
                } else {
                    vec!["worktree", "add", "-b", &branch, "--", &path_string, &base]
                };
                before_git_write(&path, &branch, &branch)?;
                run_git(&root, &args, "create the worktree")?;
                exclude_nested_worktree(&root, &path);
                return Ok(PreparedCheckout {
                    reused_worktree: false,
                    path,
                    branch: Some(branch),
                });
            }
            // A worktree is named after its branch, so one search settles both: the
            // first suffix whose branch does not exist and whose folder is free.
            let requested = branch.clone();
            let (branch, path) = free_worktree(&root, &path, &branch)?;
            let path_string = path.to_string_lossy().into_owned();
            before_git_write(&path, &branch, &requested)?;
            run_git(
                &root,
                &["worktree", "add", "-b", &branch, "--", &path_string, &base],
                "create the worktree",
            )?;
            exclude_nested_worktree(&root, &path);
            Ok(PreparedCheckout {
                reused_worktree: false,
                path,
                branch: Some(branch),
            })
        },
    }
}

/// Keep a worktree made inside the primary checkout out of that checkout's `git status`.
///
/// `.worktrees/` sits in the user's own tree, so without this `git status` lists it and
/// `git add -A` records every worktree in it as an embedded repository. The entry names the
/// worktree itself, never a folder above it: a `worktree_dir` such as `src/worktrees` must
/// not hide the rest of `src`. It goes to the repository's local `info/exclude`, never to a
/// tracked `.gitignore`, and only when nothing ignores the worktree already. A failure here
/// is logged: the worktree itself is fine.
fn exclude_nested_worktree(root: &std::path::Path, path: &std::path::Path) {
    let result = (|| -> Result<(), String> {
        let canonical = |path: &std::path::Path| {
            path.canonicalize()
                .map_err(|error| format!("cannot resolve {}: {error}", path.display()))
        };
        let (root, path) = (canonical(root)?, canonical(path)?);
        let Some(relative) = path
            .strip_prefix(&root)
            .ok()
            .map(|relative| {
                relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .filter(|relative| !relative.is_empty())
        else {
            return Ok(());
        };
        let ignored = process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["check-ignore", "-q", "--", &format!("{relative}/")])
            .status()
            .map_err(|error| format!("failed to run git: {error}"))?;
        if ignored.success() {
            return Ok(());
        }
        let output = process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--git-common-dir"])
            .output()
            .map_err(|error| format!("failed to run git: {error}"))?;
        if !output.status.success() {
            return Err(git_error("find the git directory", &output.stderr));
        }
        let info = root
            .join(String::from_utf8_lossy(&output.stdout).trim())
            .join("info");
        std::fs::create_dir_all(&info)
            .map_err(|error| format!("cannot create {}: {error}", info.display()))?;
        let exclude = info.join("exclude");
        let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude)
            .map_err(|error| format!("cannot open {}: {error}", exclude.display()))?;
        let separator = if existing.is_empty() || existing.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::io::Write::write_all(
            &mut file,
            format!(
                "{separator}# Agent worktree made by yinz\n/{}/\n",
                gitignore_literal(&relative)
            )
            .as_bytes(),
        )
        .map_err(|error| format!("cannot write {}: {error}", exclude.display()))
    })();
    if let Err(error) = result {
        log::warn!(
            "could not exclude worktree {} from git status: {error}",
            path.display()
        );
    }
}

/// `path` with its gitignore wildcards escaped, so the pattern matches only that folder.
/// The written `/{path}/` already keeps a leading `!` or `#` and trailing spaces literal.
fn gitignore_literal(path: &str) -> String {
    let mut pattern = String::with_capacity(path.len());
    for character in path.chars() {
        if matches!(character, '\\' | '*' | '?' | '[') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern
}

/// How many suffixes are tried before a request is answered with a refusal rather than
/// another `git worktree list`. A hundred agents on one branch name is not a case to serve
/// quietly; it is a case to say something about.
const FREE_NAME_ATTEMPTS: u32 = 100;

/// `name`, `name-2`, `name-3`, … — the names a collision walks through.
fn suffixed(name: &str, attempt: u32) -> String {
    if attempt == 0 {
        name.to_owned()
    } else {
        format!("{name}-{}", attempt + 1)
    }
}

/// The first of those names no branch has taken.
fn free_branch(root: &std::path::Path, branch: &str) -> Result<String, String> {
    (0..FREE_NAME_ATTEMPTS)
        .map(|attempt| suffixed(branch, attempt))
        .find(|candidate| !branch_exists(root, candidate))
        .ok_or_else(|| format!("git could not find a free branch name near {branch}"))
}

/// The first name free as *both* a branch and a folder, with the worktree renamed to match -
/// the same suffix on the branch and on the folder's last component, so the two never come
/// apart.
///
/// One listing for the whole search rather than one per candidate: the answer only has to be
/// free of what is there now, and a race with another `git worktree add` is caught by git
/// itself. A folder that is not a worktree counts as taken here rather than as the hard
/// failure it is under reuse — the point of a suffix is to step around what is in the way.
fn free_worktree(
    root: &std::path::Path,
    path: &std::path::Path,
    branch: &str,
) -> Result<(String, PathBuf), String> {
    let parent = path.parent().unwrap_or(root).to_path_buf();
    // The folder's own last component carries the suffix, not the whole branch name: a
    // branch with a `/` in it is laid out as folders (`.worktrees/feature/foo`), and
    // joining `feature/foo` onto that folder's parent would bury the worktree a level
    // deeper than the path the request was planned and confirmed with.
    let leaf = path
        .file_name()
        .map(|leaf| leaf.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} is not a usable worktree path", path.display()))?;
    let existing = worktrees(root)?;
    for attempt in 0..FREE_NAME_ATTEMPTS {
        let candidate = suffixed(branch, attempt);
        let path = parent.join(suffixed(&leaf, attempt));
        let taken = branch_exists(root, &candidate)
            || path.exists()
            || existing
                .iter()
                .any(|worktree| same_path(&worktree.path, &path));
        if !taken {
            return Ok((candidate, path));
        }
    }
    Err(format!(
        "git could not find a free worktree name near {branch}"
    ))
}

/// Whether `folder` is a *linked* worktree: a checkout of a repository that is not that
/// repository's primary one.
///
/// A folder that is no repository at all answers false rather than failing — "not a
/// worktree" is the answer being asked for, and a spawn outside a repository is exactly one
/// of the cases the caller has to own up to.
pub(super) fn linked_worktree(folder: &std::path::Path) -> Result<bool, String> {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(folder)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Ok(false);
    }
    let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let primary = primary_worktree(&root)?;
    Ok(!same_path(&root, &primary))
}

/// The repository's primary worktree — the checkout a relative worktree path is
/// resolved against.
///
/// `git rev-parse --show-toplevel` answers the *caller's* worktree, which is the
/// wrong anchor for `git worktree add`. `git worktree list --porcelain` always
/// lists the primary worktree first and answers its real checkout path. Taking the
/// parent of `--git-common-dir` instead would assume the common directory is a
/// `.git` sitting inside its checkout, which `--separate-git-dir` and `$GIT_DIR`
/// both break.
pub(super) fn primary_worktree(root: &std::path::Path) -> Result<PathBuf, String> {
    worktrees(root)?
        .into_iter()
        .next()
        .map(|worktree| worktree.path)
        .ok_or_else(|| "git could not find the primary worktree".to_owned())
}

/// One checkout of a repository: where it is, and which branch it holds.
///
/// `branch` is `None` for a detached head and for the bare entry a bare
/// repository lists first - neither can be reused for a branch request.
pub(super) struct Worktree {
    pub(super) path: PathBuf,
    pub(super) branch: Option<String>,
}

/// Every checkout of the repository, primary first.
///
/// One `git worktree list --porcelain` for all three questions asked of it
/// (which folder anchors a relative path, is this folder already a worktree, who
/// holds this branch), so a request costs one process rather than three.
pub(super) fn worktrees(root: &std::path::Path) -> Result<Vec<Worktree>, String> {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Err(git_error("list the worktrees", &output.stderr));
    }
    Ok(parse_worktrees(&String::from_utf8_lossy(&output.stdout)))
}

/// Porcelain blocks, one per checkout, separated by blank lines: a `worktree`
/// line opens each and `branch refs/heads/<name>` follows it unless the checkout
/// is detached or bare.
pub(super) fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut worktrees: Vec<Worktree> = vec![];
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if !path.is_empty() {
                worktrees.push(Worktree {
                    path: PathBuf::from(path),
                    branch: None,
                });
            }
        } else if let Some(reference) = line.strip_prefix("branch ")
            && let Some(worktree) = worktrees.last_mut()
        {
            worktree.branch = reference
                .strip_prefix("refs/heads/")
                .filter(|branch| !branch.is_empty())
                .map(str::to_owned);
        }
    }
    worktrees
}

/// The checkout already sitting at `path`, if one is - branch included, because the
/// caller reuses it as it stands and has to report the branch it holds rather than the
/// one that was asked for.
pub(super) fn worktree_at(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<Option<Worktree>, String> {
    Ok(worktrees(root)?
        .into_iter()
        .find(|worktree| same_path(&worktree.path, path)))
}

/// The checkout that has `branch` out, if any has.
fn worktree_holding(root: &std::path::Path, branch: &str) -> Result<Option<PathBuf>, String> {
    Ok(worktrees(root)?
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some(branch))
        .map(|worktree| worktree.path))
}

/// Whether `branch` is a local branch of this repository.
///
/// Exit status only: "no such branch" is an answer here, not a failure worth a
/// message, so this deliberately does not go through `run_git`.
pub(super) fn branch_exists(root: &std::path::Path, branch: &str) -> bool {
    process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Whether two paths name the same folder.
///
/// Git answers with physical paths while a requested worktree path is usually
/// one that does not exist yet, so the resolved comparison is tried first and
/// the literal one is what is left when either side cannot be resolved.
fn same_path(one: &std::path::Path, other: &std::path::Path) -> bool {
    match (std::fs::canonicalize(one), std::fs::canonicalize(other)) {
        (Ok(one), Ok(other)) => one == other,
        _ => one == other,
    }
}

fn validate_git_names(root: &std::path::Path, branch: &str, base: &str) -> Result<(), String> {
    if branch.trim().is_empty() || base.trim().is_empty() || base.starts_with('-') {
        return Err("branch and base must be non-empty, and base cannot start with '-'".to_owned());
    }
    run_git(
        root,
        &["check-ref-format", "--branch", branch],
        "validate the branch",
    )
}

pub(super) fn run_git(
    root: &std::path::Path,
    args: &[&str],
    operation: &str,
) -> Result<(), String> {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    output
        .status
        .success()
        .then_some(())
        .ok_or_else(|| git_error(operation, &output.stderr))
}

fn git_error(operation: &str, stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr).trim().to_owned();
    if detail.is_empty() {
        format!("git could not {operation}")
    } else {
        format!("git could not {operation}: {detail}")
    }
}

//! Host validation and the service-owned local project snapshot.
use crate::plugin_map::PluginEnv;
use project_data::{
    location::Location,
    projects::{Probed, Repository},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    path::Path,
    sync::{LazyLock, RwLock},
};

static LOCATIONS: LazyLock<RwLock<(Option<u64>, Probed)>> = LazyLock::new(Default::default);

pub(crate) fn resolve_directory(
    directory: &Path,
    registry: &project_data::projects::Registry,
    mut probed: Probed,
) -> Result<Option<yinz_types::data::ResolvedProject>, String> {
    let directory = directory
        .canonicalize()
        .map_err(|error| format!("resolve project directory: {error}"))?;
    if !directory.is_dir() {
        return Err("project directory must be a directory".into());
    }
    let location = Location::local(directory.to_string_lossy());
    // Remembered folders resolve without Git, just as they do in pane arrangement.
    if let Some(project) = project_data::projects::resolve(&Probed::new(), registry, &location) {
        return Ok(Some(project));
    }
    // Provider results are owned by agent-service; preserve its precedence over Git.
    if !project_data::projects::repository_for_cwd(&probed, &location)
        .is_some_and(|repo| repo.provider_identity.is_some())
    {
        let probe = yinz_process::pty_host::remote::status::probe_git(&location.path);
        let repository = match project_data::git_probe::repository(
            probe.exit_code,
            &probe.metadata,
            &probe.config,
        ) {
            project_data::git_probe::ProbeOutcome::Checkout(repository) => repository,
            project_data::git_probe::ProbeOutcome::NotARepository => {
                Repository::folder(&location.path)
            },
            project_data::git_probe::ProbeOutcome::Failed => {
                return Err("could not probe project directory".into())
            },
        };
        probed.insert(location.clone(), repository);
    }
    Ok(project_data::projects::resolve(
        &probed, registry, &location,
    ))
}

pub(crate) fn invalidate() {
    *LOCATIONS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Default::default();
}

pub(crate) fn snapshot() -> Probed {
    LOCATIONS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .1
        .clone()
}

pub(crate) fn clear(generation: u64) -> bool {
    let mut cache = LOCATIONS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.0 == Some(generation) {
        *cache = Default::default();
        true
    } else {
        false
    }
}

pub(crate) fn service(env: &PluginEnv) -> Result<u64, String> {
    if env.unavailable.load(std::sync::atomic::Ordering::Acquire)
        || env.cancel.load(std::sync::atomic::Ordering::Acquire)
    {
        return Err("project service is no longer available".into());
    }
    if !matches!(&env.plugin_config.location, yinz_types::input::layout::RunPluginLocation::Yinz(tag) if tag.to_string() == "agent-service")
    {
        return Err(
            "only the builtin agent service may publish or validate project locations".into(),
        );
    }
    env.generation
        .ok_or_else(|| "project service requires a generation".into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Match {
    identity: String,
    root: String,
    name: String,
    #[serde(default)]
    branch: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    provider: String,
    generation: u64,
    cwd: String,
    result: Match,
}

pub(crate) fn validate(input: Value) -> Result<Value, String> {
    let request: Request =
        serde_json::from_value(input).map_err(|e| format!("invalid project result: {e}"))?;
    if !yinz_types::project_provider::providers()
        .iter()
        .any(|p| p.id == request.provider && p.generation == request.generation)
    {
        return Err("project provider generation is no longer registered".into());
    }
    let result = request.result;
    for text in [&result.identity, &result.name] {
        if text.trim().is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
            return Err(
                "project identity and name must be nonempty text without control characters".into(),
            );
        }
    }
    if result
        .branch
        .as_ref()
        .is_some_and(|s| s.len() > 4096 || s.chars().any(char::is_control))
    {
        return Err("invalid project branch".into());
    }
    if !Path::new(&result.root).is_absolute() || !Path::new(&request.cwd).is_absolute() {
        return Err("project root and working directory must be absolute".into());
    }
    let root = std::fs::canonicalize(&result.root)
        .map_err(|e| format!("canonicalize project root: {e}"))?;
    let cwd = std::fs::canonicalize(&request.cwd)
        .map_err(|e| format!("canonicalize project working directory: {e}"))?;
    if !root.is_dir() || !cwd.is_dir() || !cwd.starts_with(&root) {
        return Err("project root must contain the working directory".into());
    }
    let identity =
        serde_json::to_string(&(request.provider, result.identity)).map_err(|e| e.to_string())?;
    serde_json::to_value(Repository {
        root: root.to_string_lossy().into_owned(),
        provider_identity: Some(identity),
        display_name: Some(result.name),
        branch: result.branch,
        ..Repository::default()
    })
    .map_err(|e| e.to_string())
}

pub(crate) fn publish(generation: u64, value: Value) -> Result<bool, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Publication {
        providers: Vec<yinz_types::project_provider::ProjectProvider>,
        locations: Probed,
    }
    let publication: Publication =
        serde_json::from_value(value).map_err(|e| format!("decode project publication: {e}"))?;
    let mut cache = LOCATIONS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if publication.providers != yinz_types::project_provider::providers() {
        return Err("project provider snapshot changed before publication".into());
    }
    let mut locations = publication.locations;
    locations.retain(|location: &Location, repository| {
        location.host.is_none() && repository.host.is_none()
    });
    let changed = cache.0 != Some(generation) || cache.1 != locations;
    *cache = (Some(generation), locations);
    Ok(changed)
}

#[cfg(test)]
#[path = "unit/project_provider.rs"]
mod tests;

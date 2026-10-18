//! Generation-owned project identification providers.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{LazyLock, RwLock},
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectProvider {
    pub id: String,
    pub priority: i32,
    #[serde(default)]
    pub plugin_id: u32,
    #[serde(default)]
    pub generation: u64,
}

static REGISTRY: LazyLock<RwLock<BTreeMap<String, ProjectProvider>>> =
    LazyLock::new(Default::default);

pub fn providers() -> Vec<ProjectProvider> {
    let mut providers: Vec<_> = REGISTRY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .cloned()
        .collect();
    providers.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.id.cmp(&b.id)));
    providers
}

pub fn register(
    mut provider: ProjectProvider,
    generation: u64,
    plugin_id: u32,
) -> Result<(), String> {
    if provider.id == "git"
        || !(1..=32).contains(&provider.id.len())
        || !provider
            .id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
    {
        return Err("project provider ID must be 1–32 lowercase letters, digits, underscores or hyphens; git is reserved".into());
    }
    let mut registry = REGISTRY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if registry
        .get(&provider.id)
        .is_some_and(|old| old.generation != generation)
    {
        return Err("project provider ID is already registered".into());
    }
    if registry.len() >= 128 && !registry.contains_key(&provider.id) {
        return Err("at most 128 project providers may be registered".into());
    }
    provider.generation = generation;
    provider.plugin_id = plugin_id;
    registry.insert(provider.id.clone(), provider);
    Ok(())
}

pub fn unregister_owner(generation: u64) -> bool {
    let mut registry = REGISTRY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = registry.len();
    registry.retain(|_, provider| provider.generation != generation);
    before != registry.len()
}

#[cfg(test)]
#[path = "unit/project_provider.rs"]
mod tests;

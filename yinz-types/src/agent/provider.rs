//! Agent behavior registered by plugins and shared with host and guest callers.
use crate::keys::KeyWithModifier;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
    sync::{Arc, LazyLock, RwLock},
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentProvider {
    pub kind: String,
    pub executable: String,
    pub aliases: Vec<String>,
    pub launch: AgentLaunch,
    pub restart: Option<AgentRestart>,
    /// Absent when lifecycle reporting exists without reliable prompt submission.
    pub input: Option<AgentInput>,
    pub hooks: Vec<AgentHookArtifact>,
}

/// Argument templates are argv elements, never shell commands. `{value}` is substituted
/// inside an element without splitting it. An empty template rejects that operation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentLaunch {
    pub args: Vec<String>,
    pub model_args: Vec<String>,
    pub prompt_args: Vec<String>,
    pub resume_args: Vec<String>,
    pub resume_last_args: Vec<String>,
    pub resume_before_args: bool,
    pub permissions: BTreeMap<String, Vec<String>>,
    pub permission_descriptions: BTreeMap<String, String>,
    pub efforts: BTreeMap<String, Vec<String>>,
}

/// Explicit argv grammar for restarting without replaying the previous prompt/session.
/// Unknown options and ambiguous positionals are errors.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentRestart {
    pub values: Vec<String>,
    pub switches: Vec<String>,
    pub inline_values: Vec<String>,
    pub forbidden_commands: Vec<String>,
    pub remove_values: Vec<String>,
    pub remove_switches: Vec<String>,
    pub remove_optional_values: Vec<String>,
    pub keep_positionals: bool,
    pub max_positionals: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentInput {
    /// Clears the whole composer after a marker paste, before each explicit prompt.
    pub clear_key: String,
    pub submit_key: String,
    pub submit_delay_ms: u64,
    pub paste_prefix: String,
    pub command_prefixes: Vec<String>,
    pub newline: String,
}

impl Default for AgentInput {
    fn default() -> Self {
        Self {
            clear_key: "Ctrl c".into(),
            submit_key: "Enter".into(),
            submit_delay_ms: 100,
            paste_prefix: String::new(),
            command_prefixes: vec!["/".into()],
            newline: "\n".into(),
        }
    }
}

impl AgentInput {
    pub fn is_command_prompt(&self, prompt: &str) -> bool {
        self.command_prefixes
            .iter()
            .any(|prefix| prompt.trim_start().starts_with(prefix))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHookBase {
    #[default]
    Config,
    Home,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentHookArtifact {
    pub base: AgentHookBase,
    pub path: String,
    pub content: String,
    pub marker: String,
    pub executable: bool,
}

impl AgentProvider {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_kind(&self.kind) || super::KINDS.contains(&self.kind.as_str()) {
            return Err("agent provider kind must be a new lowercase name of 1–32 letters, digits, underscores or hyphens".into());
        }
        if self.executable.is_empty()
            || self.executable.len() > 4096
            || self.executable.chars().any(char::is_control)
        {
            return Err(
                "agent provider executable must be a nonempty path without control characters"
                    .into(),
            );
        }
        if self.aliases.len() > 64 || self.aliases.iter().any(|alias| !valid_kind(alias)) {
            return Err("agent provider aliases must be lowercase agent names (at most 64)".into());
        }
        for name in self.names() {
            let normalized = name.to_ascii_lowercase();
            let name = normalized.as_str();
            let stem = name.split('.').next().unwrap_or_default();
            if [name, stem].iter().any(|candidate| {
                super::SHELLS.contains(candidate)
                    || super::hooks::INTERPRETERS.contains(candidate)
                    || matches!(*candidate, "env" | "yinz")
            }) {
                return Err(format!(
                    "agent provider cannot identify generic shell or interpreter {name:?}"
                ));
            }
            for candidate in [name, stem, stem.split('-').next().unwrap_or_default()] {
                if super::KINDS.contains(&candidate) || candidate == "antigravity" {
                    return Err(format!(
                        "agent provider name {name:?} conflicts with a builtin"
                    ));
                }
            }
        }
        let templates = [
            &self.launch.args,
            &self.launch.model_args,
            &self.launch.prompt_args,
            &self.launch.resume_args,
            &self.launch.resume_last_args,
        ];
        for args in templates
            .into_iter()
            .chain(self.launch.permissions.values())
            .chain(self.launch.efforts.values())
        {
            if args.len() > 128
                || args
                    .iter()
                    .any(|arg| arg.len() > 8192 || arg.contains('\0'))
            {
                return Err("agent argument templates exceed their limits or contain NUL".into());
            }
        }
        if self.launch.permissions.len() > 64
            || self.launch.efforts.len() > 64
            || self
                .launch
                .permissions
                .keys()
                .chain(self.launch.efforts.keys())
                .any(|name| {
                    name.is_empty() || name.len() > 64 || name.chars().any(char::is_control)
                })
        {
            return Err("agent option names exceed their limits".into());
        }
        if self
            .launch
            .permission_descriptions
            .iter()
            .any(|(mode, description)| {
                !self.launch.permissions.contains_key(mode)
                    || description.len() > 128
                    || description.chars().any(char::is_control)
            })
        {
            return Err(
                "agent permission descriptions must name a declared mode and fit in 128 characters"
                    .into(),
            );
        }
        if let Some(restart) = &self.restart {
            let groups = [
                &restart.values,
                &restart.switches,
                &restart.inline_values,
                &restart.remove_values,
                &restart.remove_switches,
                &restart.remove_optional_values,
            ];
            let mut seen = std::collections::BTreeSet::new();
            for flags in groups {
                if flags.len() > 256
                    || flags.iter().any(|flag| {
                        !flag.starts_with('-')
                            || flag.len() < 2
                            || flag.len() > 128
                            || flag.contains(['=', ' ', '\t', '\n', '\r', '\0'])
                            || !seen.insert(flag)
                    })
                {
                    return Err("agent restart flags must be distinct bounded option names".into());
                }
            }
            if restart.max_positionals > 128
                || restart.forbidden_commands.len() > 256
                || restart.forbidden_commands.iter().any(|command| {
                    command.is_empty()
                        || command.len() > 128
                        || command.chars().any(char::is_control)
                })
            {
                return Err("agent restart grammar exceeds its limits".into());
            }
        }
        if let Some(input) = &self.input {
            if input.clear_key.parse::<KeyWithModifier>().is_err()
                || input.submit_key.parse::<KeyWithModifier>().is_err()
                || input.submit_delay_ms > 10_000
                || input.paste_prefix.len() > 4096
                || input.paste_prefix.chars().any(char::is_control)
                || input.command_prefixes.len() > 32
                || input.command_prefixes.iter().any(|prefix| {
                    prefix.is_empty() || prefix.len() > 32 || prefix.chars().any(char::is_control)
                })
                || !matches!(input.newline.as_str(), "\n" | "\r" | "\r\n")
            {
                return Err("agent input settings are invalid or exceed their limits".into());
            }
        }
        if self.hooks.len() > 16 {
            return Err("agent provider has too many hook artifacts".into());
        }
        for hook in &self.hooks {
            if hook.path.is_empty()
                || hook.path.len() > 4096
                || Path::new(&hook.path)
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
                || hook.path.contains('\\')
                || hook.path.chars().any(char::is_control)
                || hook.marker.is_empty()
                || hook.marker.len() > 256
                || hook.marker.chars().any(char::is_control)
                || hook.content.len() > 1024 * 1024
                || hook.content.contains('\0')
                || !hook.content.contains(&hook.marker)
            {
                return Err("agent hook artifacts require a relative path, a marker present in the content, and at most 1 MiB of text".into());
            }
        }
        if serde_json::to_vec(self)
            .map_err(|error| format!("serialize agent provider: {error}"))?
            .len()
            > 2 * 1024 * 1024
        {
            return Err("agent provider exceeds 2 MiB".into());
        }
        Ok(())
    }

    fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.kind.as_str())
            .chain(std::iter::once(super::program(&self.executable)))
            .chain(self.aliases.iter().map(String::as_str))
    }
}

fn valid_kind(kind: &str) -> bool {
    (1..=32).contains(&kind.len())
        && kind
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
}

/// Expand one argv template without interpreting shell syntax.
pub fn expand_args(template: &[String], value: &str) -> Vec<String> {
    template
        .iter()
        .map(|arg| arg.replace("{value}", value))
        .collect()
}

type Registry = BTreeMap<String, (u64, Arc<AgentProvider>)>;
const MAX_REGISTRY_BYTES: usize = 8 * 1024 * 1024;

static REGISTRY: LazyLock<RwLock<Registry>> = LazyLock::new(|| RwLock::new(BTreeMap::new()));

thread_local! {
    static MODEL_SNAPSHOT: std::cell::RefCell<Option<Vec<Arc<AgentProvider>>>> = const { std::cell::RefCell::new(None) };
}

/// Evaluate a pure model against an explicit provider snapshot, without changing the host registry.
/// Nesting and unwinding restore the caller's context; other threads never observe it.
pub fn with_snapshot<T>(providers: &[AgentProvider], evaluate: impl FnOnce() -> T) -> T {
    struct Restore(Option<Vec<Arc<AgentProvider>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            MODEL_SNAPSHOT.with(|snapshot| *snapshot.borrow_mut() = self.0.take());
        }
    }
    let previous = MODEL_SNAPSHOT
        .with(|snapshot| snapshot.replace(Some(providers.iter().cloned().map(Arc::new).collect())));
    let _restore = Restore(previous);
    evaluate()
}

/// Find canonical kinds, executable names, or declared aliases. Returned snapshots remain
/// valid after the owning plugin reloads or unloads.
pub fn provider(kind: &str) -> Option<Arc<AgentProvider>> {
    let name = super::program(kind).to_ascii_lowercase();
    if let Some(found) = MODEL_SNAPSHOT.with(|snapshot| {
        snapshot.borrow().as_ref().map(|providers| {
            providers
                .iter()
                .find(|provider| {
                    provider
                        .names()
                        .any(|candidate| candidate.eq_ignore_ascii_case(&name))
                })
                .cloned()
        })
    }) {
        return found;
    }
    let registry = REGISTRY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    registry.values().find_map(|(_, provider)| {
        provider
            .names()
            .any(|candidate| candidate.eq_ignore_ascii_case(&name))
            .then(|| Arc::clone(provider))
    })
}

pub fn providers() -> Vec<AgentProvider> {
    if let Some(providers) = MODEL_SNAPSHOT.with(|snapshot| {
        snapshot.borrow().as_ref().map(|providers| {
            providers
                .iter()
                .map(|provider| provider.as_ref().clone())
                .collect()
        })
    }) {
        return providers;
    }
    REGISTRY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .map(|(_, provider)| provider.as_ref().clone())
        .collect()
}

pub fn register(owner: u64, provider: AgentProvider) -> Result<(), String> {
    provider.validate()?;
    let mut registry = REGISTRY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    insert(&mut registry, owner, provider)
}

fn insert(registry: &mut Registry, owner: u64, provider: AgentProvider) -> Result<(), String> {
    if registry.len() >= 128 && !registry.contains_key(&provider.kind) {
        return Err("at most 128 agent providers may be registered".into());
    }
    for (kind, (existing_owner, existing)) in registry.iter() {
        if kind == &provider.kind && *existing_owner == owner {
            continue;
        }
        if provider.names().any(|name| {
            existing
                .names()
                .any(|other| name.eq_ignore_ascii_case(other))
        }) {
            return Err(format!(
                "agent provider conflicts with registered provider {kind}"
            ));
        }
    }
    let encoded_len = |provider: &AgentProvider| {
        serde_json::to_vec(provider)
            .map(|encoded| encoded.len() + 1)
            .map_err(|error| format!("serialize agent provider: {error}"))
    };
    let mut bytes = encoded_len(&provider)? + 2;
    for (kind, (_, existing)) in registry.iter() {
        if kind != &provider.kind {
            bytes += encoded_len(existing)?;
        }
        if bytes > MAX_REGISTRY_BYTES {
            return Err("agent provider registry exceeds 8 MiB".into());
        }
    }
    registry.insert(provider.kind.clone(), (owner, Arc::new(provider)));
    Ok(())
}

/// Owner zero is reserved for host snapshots; running plugins use their generation.
pub fn unregister_owner(owner: u64) -> bool {
    let mut registry = REGISTRY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = registry.len();
    if owner != 0 {
        registry.retain(|_, (registered_owner, _)| *registered_owner != owner);
    }
    registry.len() != before
}

/// Atomically replace the local descriptor snapshot.
pub fn replace_snapshot(providers: Vec<AgentProvider>) -> Result<(), String> {
    let mut registry = BTreeMap::new();
    for provider in providers {
        provider.validate()?;
        if registry.contains_key(&provider.kind) {
            return Err(format!(
                "duplicate agent provider {} in snapshot",
                provider.kind
            ));
        }
        insert(&mut registry, 0, provider)?;
    }
    *REGISTRY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = registry;
    Ok(())
}

#[cfg(test)]
#[path = "unit/provider_tests.rs"]
mod tests;

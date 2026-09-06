//! Lua plugins that add tools to Hocus.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(target_arch = "wasm32")]
use zellij_tile::prelude::MessageToPlugin;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const DISCOVER: &str = "hocus_extension_discover";
pub const REGISTER: &str = "hocus_extension_register";
/// A tool call travels as a `zellij.rpc` call, which the prelude answers for the extension: the
/// extension's `zellij.hocus.tools { name = { ..., handler = fn } }` registers a handler named
/// `hocus_tool:<name>`, and the reply comes back on the rpc reply message, addressed to this
/// plugin's id.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const CALL: &str = "zellij_rpc_call";
pub const RESULT: &str = "zellij_rpc_reply";
/// The rpc handler prefix hocus calls a tool under.
pub const TOOL_HANDLER_PREFIX: &str = "hocus_tool:";
/// How long a call may go unanswered before hocus gives up on it: an extension that raised
/// before replying, unloaded mid-call, or never had a handler for the name.
pub const CALL_TIMEOUT_MS: u64 = 30_000;
pub const SCAN_CONTEXT: &str = "hocus_extension_scan";
pub const VERSION: u32 = 1;

const MAX_REGISTRATION_BYTES: usize = 64_000;
pub(crate) const MAX_RESULT_BYTES: usize = 64_000;
const MAX_TOOLS_PER_EXTENSION: usize = 32;
const MAX_DESCRIPTION_BYTES: usize = 2_000;
const MAX_SCHEMA_BYTES: usize = 16_000;
const MAX_TOOL_NAME_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Safety {
    ReadOnly,
    Automatic,
    Confirm,
}

#[derive(Debug, Deserialize)]
struct Registration {
    version: u32,
    namespace: String,
    path: String,
    tools: Vec<RegistrationTool>,
}

#[derive(Debug, Deserialize)]
struct RegistrationTool {
    name: String,
    description: String,
    parameters: Value,
    safety: Safety,
    #[serde(default)]
    consequence: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExtensionTool {
    pub owner: zellij_tile::prelude::PluginId,
    pub path: String,
    pub namespace: String,
    pub local_name: String,
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub safety: Safety,
    pub consequence: Option<String>,
}

impl ExtensionTool {
    pub fn spec(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            },
        })
    }
}

#[derive(Default)]
pub struct Registry {
    tools: BTreeMap<String, ExtensionTool>,
    owners: BTreeMap<zellij_tile::prelude::PluginId, BTreeSet<String>>,
    identities: BTreeMap<zellij_tile::prelude::PluginId, (String, String)>,
}

impl Registry {
    pub fn tool(&self, name: &str) -> Option<&ExtensionTool> {
        self.tools.get(name)
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn specs(&self, read_only: bool) -> Vec<Value> {
        self.tools
            .values()
            .filter(|tool| !read_only || tool.safety == Safety::ReadOnly)
            .map(ExtensionTool::spec)
            .collect()
    }

    pub fn owner_for_path(&self, path: &str) -> Option<zellij_tile::prelude::PluginId> {
        self.identities
            .iter()
            .find(|(_, (_, registered_path))| registered_path == path)
            .map(|(owner, _)| *owner)
    }

    pub fn remove_owner(&mut self, owner: zellij_tile::prelude::PluginId) {
        self.identities.remove(&owner);
        if let Some(names) = self.owners.remove(&owner) {
            for name in names {
                self.tools.remove(&name);
            }
        }
    }

    pub fn register(
        &mut self,
        owner: zellij_tile::prelude::PluginId,
        payload: &str,
        discovered: &BTreeMap<String, String>,
        instances: &BTreeMap<zellij_tile::prelude::PluginId, (String, String)>,
        reserved: &BTreeSet<String>,
    ) -> Result<Vec<String>, String> {
        if payload.len() > MAX_REGISTRATION_BYTES {
            return Err(format!(
                "extension registration is {} bytes; the limit is {MAX_REGISTRATION_BYTES}",
                payload.len()
            ));
        }
        let registration: Registration = serde_json::from_str(payload)
            .map_err(|error| format!("invalid extension registration: {error}"))?;
        if registration.version != VERSION {
            return Err(format!(
                "unsupported Hocus extension protocol {}; expected {VERSION}",
                registration.version
            ));
        }
        if discovered.get(&registration.namespace) != Some(&registration.path) {
            return Err(format!(
                "extension {:?} is not a discovered hocus/*.lua file",
                registration.namespace
            ));
        }
        if !instances.get(&owner).is_some_and(|(namespace, path)| {
            namespace == &registration.namespace && path == &registration.path
        }) {
            return Err(format!(
                "plugin {owner} does not own extension {:?}",
                registration.namespace
            ));
        }
        if registration.tools.len() > MAX_TOOLS_PER_EXTENSION {
            return Err(format!(
                "extension {:?} registered {} tools; the limit is {MAX_TOOLS_PER_EXTENSION}",
                registration.namespace,
                registration.tools.len()
            ));
        }

        let mut replacement = Vec::with_capacity(registration.tools.len());
        let mut seen = BTreeSet::new();
        for tool in registration.tools {
            if !valid_name(&tool.name) {
                return Err(format!(
                    "extension tool {:?} must be lower snake case",
                    tool.name
                ));
            }
            let name = format!("{}_{}", registration.namespace, tool.name);
            if name.len() > MAX_TOOL_NAME_BYTES {
                return Err(format!("extension tool name {name:?} is too long"));
            }
            if reserved.contains(&name) {
                return Err(format!(
                    "extension tool {name:?} collides with a Hocus command"
                ));
            }
            if !seen.insert(name.clone()) {
                return Err(format!("extension registered {name:?} more than once"));
            }
            if self
                .tools
                .get(&name)
                .is_some_and(|existing| existing.owner != owner)
            {
                return Err(format!("extension tool {name:?} is already registered"));
            }
            let description = tool.description.trim().to_owned();
            if description.is_empty() || description.len() > MAX_DESCRIPTION_BYTES {
                return Err(format!(
                    "extension tool {name:?} needs a description no longer than {MAX_DESCRIPTION_BYTES} bytes"
                ));
            }
            if tool.parameters.get("type").and_then(Value::as_str) != Some("object") {
                return Err(format!(
                    "extension tool {name:?} parameters must be a JSON schema with type=object"
                ));
            }
            if tool.parameters.to_string().len() > MAX_SCHEMA_BYTES {
                return Err(format!("extension tool {name:?} schema is too large"));
            }
            let consequence = tool
                .consequence
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty());
            if tool.safety == Safety::Confirm && consequence.is_none() {
                return Err(format!(
                    "confirming extension tool {name:?} needs a consequence"
                ));
            }
            replacement.push(ExtensionTool {
                owner,
                path: registration.path.clone(),
                namespace: registration.namespace.clone(),
                local_name: tool.name,
                name,
                description,
                parameters: tool.parameters,
                safety: tool.safety,
                consequence,
            });
        }

        self.remove_owner(owner);
        let mut names = BTreeSet::new();
        for tool in replacement {
            names.insert(tool.name.clone());
            self.tools.insert(tool.name.clone(), tool);
        }
        self.owners.insert(owner, names.clone());
        self.identities.insert(
            owner,
            (registration.namespace.clone(), registration.path.clone()),
        );
        Ok(names.into_iter().collect())
    }
}

#[derive(Debug, Serialize)]
pub struct Call<'a> {
    pub version: u32,
    pub call_id: &'a str,
    /// `hocus_tool:<local name>`.
    pub name: String,
    /// The tool's arguments, as the model sent them.
    pub payload: Value,
}

/// The rpc reply: `result` from a handler that returned, `error` from one that raised or from
/// the prelude when nothing handles the name.
#[derive(Debug, Deserialize)]
pub struct ToolResult {
    pub version: u32,
    pub call_id: String,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

impl ToolResult {
    /// What the model is told: the result as text, or the failure named as one.
    pub fn text(&self) -> String {
        match (&self.result, &self.error) {
            (_, Some(error)) => format!("failed: {error}"),
            (Some(Value::String(text)), None) => text.clone(),
            (Some(value), None) => value.to_string(),
            (None, None) => String::new(),
        }
    }
}

pub fn extension_files(dir: &str, listing: &str) -> BTreeMap<String, String> {
    let prefix = format!("{}/hocus/", dir.trim_end_matches('/'));
    listing
        .lines()
        .filter_map(|path| {
            let stem = path.strip_prefix(&prefix)?.strip_suffix(".lua")?;
            (!stem.contains('/') && valid_name(stem)).then(|| (stem.to_owned(), path.to_owned()))
        })
        .collect()
}

pub fn namespace_for_path<'a>(dir: &str, path: &'a str) -> Option<&'a str> {
    let prefix = format!("{}/hocus/", dir.trim_end_matches('/'));
    let stem = path.strip_prefix(&prefix)?.strip_suffix(".lua")?;
    (!stem.contains('/') && valid_name(stem)).then_some(stem)
}

#[cfg(target_arch = "wasm32")]
pub fn discover() {
    zellij_tile::shim::pipe_message_to_plugin(MessageToPlugin::new(DISCOVER));
}

#[cfg(not(target_arch = "wasm32"))]
pub fn discover() {}

#[cfg(target_arch = "wasm32")]
pub fn load(path: &str, namespace: &str) {
    zellij_tile::shim::load_new_plugin(
        format!("file:{path}"),
        BTreeMap::from([
            ("hocus_extension_name".to_owned(), namespace.to_owned()),
            ("hocus_extension_path".to_owned(), path.to_owned()),
            ("shared".to_owned(), "true".to_owned()),
        ]),
        true,
        true,
    );
}

/// Off wasm a load or reload is only counted, so a test can see it was asked exactly once;
/// see `tools::test_seams`.
#[cfg(not(target_arch = "wasm32"))]
pub mod test_seams {
    use std::cell::Cell;

    thread_local! {
        pub static EXTENSION_LOAD_CALLS: Cell<usize> = const { Cell::new(0) };
    }

    pub fn count_load() {
        let calls = EXTENSION_LOAD_CALLS.get() + 1;
        EXTENSION_LOAD_CALLS.set(calls);
        assert!(
            calls <= crate::tools::test_seams::MAX_RELOAD_CALLS,
            "an extension was (re)loaded {calls} times: pump_tools is re-running a deferred head"
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load(_path: &str, _namespace: &str) {
    test_seams::count_load();
}

#[cfg(target_arch = "wasm32")]
pub fn reload(owner: zellij_tile::prelude::PluginId) {
    zellij_tile::shim::reload_plugin_with_id(owner);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn reload(_owner: zellij_tile::prelude::PluginId) {
    test_seams::count_load();
}

#[cfg(target_arch = "wasm32")]
pub fn call(owner: zellij_tile::prelude::PluginId, payload: String) {
    zellij_tile::shim::pipe_message_to_plugin(
        MessageToPlugin::new(CALL)
            .with_destination_plugin_id(owner)
            .with_payload(payload),
    );
}

#[cfg(not(target_arch = "wasm32"))]
pub fn call(_owner: zellij_tile::prelude::PluginId, _payload: String) {}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
#[path = "./unit/extensions_tests.rs"]
mod tests;

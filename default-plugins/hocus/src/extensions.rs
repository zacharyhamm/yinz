//! Lua plugins that add tools to Hocus.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(target_arch = "wasm32")]
use zellij_tile::prelude::MessageToPlugin;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const DISCOVER: &str = "hocus_extension_discover";
pub const REGISTER: &str = "hocus_extension_register";
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const CALL: &str = "hocus_extension_call";
pub const RESULT: &str = "hocus_extension_result";
pub const SCAN_CONTEXT: &str = "hocus_extension_scan";
pub const VERSION: u32 = 1;

const MAX_REGISTRATION_BYTES: usize = 64_000;
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
    pub owner: u32,
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
    owners: BTreeMap<u32, BTreeSet<String>>,
    identities: BTreeMap<u32, (String, String)>,
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

    pub fn owner_for_path(&self, path: &str) -> Option<u32> {
        self.identities
            .iter()
            .find(|(_, (_, registered_path))| registered_path == path)
            .map(|(owner, _)| *owner)
    }

    pub fn remove_owner(&mut self, owner: u32) {
        self.identities.remove(&owner);
        if let Some(names) = self.owners.remove(&owner) {
            for name in names {
                self.tools.remove(&name);
            }
        }
    }

    pub fn register(
        &mut self,
        owner: u32,
        payload: &str,
        discovered: &BTreeMap<String, String>,
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
    pub tool: &'a str,
    pub arguments: Value,
    pub reply_to: u32,
}

#[derive(Debug, Deserialize)]
pub struct ToolResult {
    pub version: u32,
    pub call_id: String,
    pub result: String,
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

#[cfg(not(target_arch = "wasm32"))]
pub fn load(_path: &str, _namespace: &str) {}

#[cfg(target_arch = "wasm32")]
pub fn reload(owner: u32) {
    zellij_tile::shim::reload_plugin_with_id(owner);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn reload(_owner: u32) {}

#[cfg(target_arch = "wasm32")]
pub fn call(owner: u32, payload: String) {
    zellij_tile::shim::pipe_message_to_plugin(
        MessageToPlugin::new(CALL)
            .with_destination_plugin_id(owner)
            .with_payload(payload),
    );
}

#[cfg(not(target_arch = "wasm32"))]
pub fn call(_owner: u32, _payload: String) {}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
#[path = "./unit/extensions_tests.rs"]
mod tests;

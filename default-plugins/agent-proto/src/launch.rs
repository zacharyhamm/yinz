//! Starting a coding agent from a agent plugin.
//!
//! The planner — what to run, and what Git setup to do first — lives in
//! `yinz_runtime::agent::launch` so the server can answer a TypeScript plugin's `yinz.agent.spawn`
//! with the very same code; it is re-exported here whole, so every caller in this crate and
//! its plugins reads `launch::plan`, `launch::Spec` and the permission-mode tables as before.
//! What stays here is what only a wasm plugin can do or need: the host call, the VM naming,
//! and the agent pane aliases.
//!
//! [`open`] cannot be pure — it is a host call — so it is gated the way [`crate::store::Host`]
//! is, with a stub on the host triple that fails loudly rather than quietly answering nothing.

pub use yinz_tile::agent::launch::*;

use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};
use yinz_tile::prelude::{SpawnedAgent, AGENT_PANE_CONTEXT};

/// A VM used by exactly one automatic agent when no worktree will be mirrored.
pub fn unique_firecracker_vm() -> String {
    thread_local! {
        static SEQUENCE: Cell<u64> = const { Cell::new(0) };
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = SEQUENCE.with(|sequence| {
        let next = sequence.get().wrapping_add(1);
        sequence.set(next);
        next
    });
    format!("yinz-agent-{now:x}-{sequence:x}")
}

// --------------------------------------------------------------- wasm: the host call

/// Do the Git setup and open the pane: `PluginCommand::SpawnAgent`, which runs the same
/// planner on the host, makes the checkout, opens the pane and records it as an agent's.
///
/// Always with a repository, and so always with a folder for a Firecracker VM to be named
/// after and mirror: the modal's VM name is a placeholder the host replaces.
#[cfg(target_family = "wasm")]
pub fn open(spec: &Spec, source_directory: std::path::PathBuf) -> Result<SpawnedAgent, String> {
    yinz_tile::shim::spawn_agent(
        spec.clone(),
        Some(source_directory),
        None,
        None,
        std::collections::BTreeMap::new(),
        true,
        false,
    )
}

/// [`open`] on the host triple, where there is no host to call.
///
/// Both callers are compiled natively to run their tests, so this has to exist on
/// both sides. It fails rather than pretending, for the same reason
/// [`crate::store::Host`]'s stub does: a test that reaches here has left the pure
/// half, and should say so instead of passing against a pane that was never opened.
#[cfg(not(target_family = "wasm"))]
pub fn open(_spec: &Spec, _source_directory: std::path::PathBuf) -> Result<SpawnedAgent, String> {
    Err("agents cannot be started outside yinz".to_owned())
}

/// The plugin alias for each of the two agent panes.
///
/// Aliases rather than `yinz:` urls: plugin identity is (location, configuration), an alias
/// contributes its own block - `shared true` - to that configuration, and the keybindings open
/// both halves by alias. A second spelling would open a second pane rather than reach the one
/// already there.
/// The context that marks a command pane as an agent's.
///
/// The host reads this off the pane's originating plugin when the command exits, and it is
/// what tells `agent_exit` apart from an ordinary command pane holding on its exit code.
/// One spelling, here, because every route that starts an agent has to carry it: the two
/// host calls that set it themselves, and `agent_spawn`'s no-checkout route, which opens an
/// ordinary command pane and so has to say so.
pub fn agent_pane_context() -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::from([(AGENT_PANE_CONTEXT.to_owned(), "true".to_owned())])
}

pub const AGENTS_ALIAS: &str = "agents";

/// Match the Agents panel's keybinding configuration so plugin launches reuse its instance.
/// `awaits_placement` keeps it hidden until Agents resolves its geometry.
pub fn overlay_configuration() -> std::collections::BTreeMap<String, String> {
    let mut configuration = std::collections::BTreeMap::new();
    configuration.insert("overlay".to_owned(), "true".to_owned());
    configuration.insert("awaits_placement".to_owned(), "true".to_owned());
    configuration
}

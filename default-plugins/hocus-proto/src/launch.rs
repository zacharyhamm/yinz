//! Starting a coding agent from a hocus plugin.
//!
//! The planner — what to run, and what Git setup to do first — lives in
//! `zellij_utils::agent::launch` so the server can answer a Lua plugin's `zellij.agent.spawn`
//! with the very same code; it is re-exported here whole, so every caller in this crate and
//! its plugins reads `launch::plan`, `launch::Spec` and the permission-mode tables as before.
//! What stays here is what only a wasm plugin can do or need: the host call, the VM naming,
//! and the hocus pane aliases.
//!
//! [`open`] cannot be pure — it is a host call — so it is gated the way [`crate::store::Host`]
//! is, with a stub on the host triple that fails loudly rather than quietly answering nothing.

pub use zellij_tile::agent::launch::*;

use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};
use zellij_tile::prelude::{PaneId, HOCUS_AGENT_PANE_CONTEXT};

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
    format!("hocus-agent-{now:x}-{sequence:x}")
}

// --------------------------------------------------------------- wasm: the host call

/// Do the Git setup and open the pane: `PluginCommand::SpawnAgent`, which runs the same
/// planner on the host, makes the checkout, opens the pane and records it as an agent's.
///
/// Always with a repository, and so always with a folder for a Firecracker VM to be named
/// after and mirror: the modal's VM name is a placeholder the host replaces.
#[cfg(target_family = "wasm")]
pub fn open(spec: &Spec, source_directory: std::path::PathBuf) -> Result<PaneId, String> {
    zellij_tile::shim::spawn_agent(
        spec.clone(),
        Some(source_directory),
        None,
        None,
        std::collections::BTreeMap::new(),
        true,
    )
}

/// [`open`] on the host triple, where there is no host to call.
///
/// Both callers are compiled natively to run their tests, so this has to exist on
/// both sides. It fails rather than pretending, for the same reason
/// [`crate::store::Host`]'s stub does: a test that reaches here has left the pure
/// half, and should say so instead of passing against a pane that was never opened.
#[cfg(not(target_family = "wasm"))]
pub fn open(_spec: &Spec, _source_directory: std::path::PathBuf) -> Result<PaneId, String> {
    Err("agents cannot be started outside zellij".to_owned())
}

/// The plugin alias for each of the two hocus panes.
///
/// Aliases rather than `zellij:` urls: plugin identity is (location, configuration), an alias
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
    std::collections::BTreeMap::from([(HOCUS_AGENT_PANE_CONTEXT.to_owned(), "true".to_owned())])
}

pub const HOCUS_ALIAS: &str = "hocus";
pub const HOCUS_AGENTS_ALIAS: &str = "hocus-agents";

/// The configuration that makes a launched hocus pane an overlay.
///
/// One definition for both halves and both directions. `LaunchOrFocusPlugin { overlay true;
/// awaits_placement true }` writes exactly these keys, so a launch from a plugin has to match them
/// character for character or the keybinding and the plugin address two different instances - and
/// the failure looks like a duplicate pane rather than an error.
///
/// `awaits_placement` is here for that reason and not because a plugin-opened half needs it any
/// less: it says "do not paint me at the default geometry I was born with, my owner will place
/// me", which is what keeps a launched panel from flashing in the middle of the screen on top of
/// its other half. Both halves place themselves through hocus's `apply_overlay_geometry`, which
/// is what releases it.
pub fn overlay_configuration() -> std::collections::BTreeMap<String, String> {
    let mut configuration = std::collections::BTreeMap::new();
    configuration.insert("overlay".to_owned(), "true".to_owned());
    configuration.insert("awaits_placement".to_owned(), "true".to_owned());
    configuration
}

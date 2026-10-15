//! Pure agent launch planning and pane aliases shared by the host and Scheme model bridge.
pub use yinz_tile::agent::launch::*;
use yinz_tile::prelude::AGENT_PANE_CONTEXT;

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

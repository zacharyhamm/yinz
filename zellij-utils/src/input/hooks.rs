//! Hooks: named points where zellij hands control to a plugin.
//!
//! A hook point is a moment in the session's life that a plugin can own - the first, and so
//! far only, one being the moment an agent pane's command exits. Before this existed, that
//! moment was a branch in the terminal emulator: `render_first_run_banner` painted a fixed
//! question and `TerminalPane::adjust_input_to_terminal` implemented its two answers. Nothing
//! could be added to it without editing the pane.
//!
//! A hook point takes **one** plugin, not a list. Two plugins cannot both replace the pane,
//! and a precedence rule between them would be a worse thing to explain than the restriction:
//! a hook that wants to do several things does them itself.
//!
//! ```kdl
//! hooks {
//!     agent_exit "file:~/.config/zellij/hooks/agent-exit.lua"
//! }
//! ```
//!
//! An empty string disables a hook point, leaving the unhooked behaviour in its place.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::layout::RunPluginOrAlias;

/// A moment a plugin can be given control of.
///
/// The KDL spelling is the snake_case name; `FromStr`/`Display` are the whole of the mapping,
/// so adding a point is adding a variant and its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum HookPoint {
    /// An agent pane's command exited and the pane is holding. The hook plugin is opened in
    /// place of that pane, which is suppressed behind it rather than closed.
    AgentExit,
}

impl HookPoint {
    pub const ALL: &'static [HookPoint] = &[HookPoint::AgentExit];

    pub fn as_str(&self) -> &'static str {
        match self {
            HookPoint::AgentExit => "agent_exit",
        }
    }
}

impl fmt::Display for HookPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for HookPoint {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "agent_exit" | "agent-exit" => Ok(HookPoint::AgentExit),
            _ => Err(format!(
                "no such hook point: '{}'. Known hook points: {}",
                s,
                HookPoint::ALL
                    .iter()
                    .map(|point| point.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

/// The plugin bound to each hook point.
///
/// Ordered rather than hashed for the same reason `Config::background_plugins` is: serializing
/// the config has to produce the same bytes every run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hooks {
    hooks: BTreeMap<HookPoint, RunPluginOrAlias>,
}

/// Deliberately not the empty map. `Default` here means "the hooks a session has when nothing
/// says otherwise", and a session with no config file still shows the agent-exit screen - it
/// did when the terminal emulator painted it, and nothing about moving it into a plugin should
/// change that.
impl Default for Hooks {
    fn default() -> Self {
        Hooks::defaults()
    }
}

impl Hooks {
    /// The hooks a session has when the config says nothing: the shipped `agent_exit` screen,
    /// which is what the terminal emulator used to paint itself.
    pub fn defaults() -> Self {
        let mut hooks = BTreeMap::new();
        if let Ok(plugin) = RunPluginOrAlias::from_url("zellij:agent-exit", &None, None, None) {
            hooks.insert(HookPoint::AgentExit, plugin);
        }
        Hooks { hooks }
    }

    pub fn from_data(hooks: BTreeMap<HookPoint, RunPluginOrAlias>) -> Self {
        Hooks { hooks }
    }

    pub fn get(&self, point: HookPoint) -> Option<&RunPluginOrAlias> {
        self.hooks.get(&point)
    }

    pub fn inner(&self) -> &BTreeMap<HookPoint, RunPluginOrAlias> {
        &self.hooks
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn set(&mut self, point: HookPoint, plugin: RunPluginOrAlias) {
        self.hooks.insert(point, plugin);
    }

    /// Unbind a hook point, which is what an empty string in the config means.
    pub fn clear(&mut self, point: HookPoint) {
        self.hooks.remove(&point);
    }

    /// Later config wins per hook point, and a point the other config does not mention keeps
    /// what it had - the same additive rule the rest of the config blocks follow.
    pub fn merge(&mut self, other: Self) {
        self.hooks.extend(other.hooks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_hooks_bind_the_shipped_agent_exit_screen() {
        let hooks = Hooks::default();
        assert_eq!(
            hooks.get(HookPoint::AgentExit).map(|p| p.location_string()),
            Some("zellij:agent-exit".to_owned()),
            "a session with no config still shows the agent-exit screen"
        );
    }

    #[test]
    fn a_hook_point_round_trips_through_its_name() {
        for point in HookPoint::ALL {
            assert_eq!(HookPoint::from_str(point.as_str()).as_ref(), Ok(point));
        }
    }

    #[test]
    fn an_unknown_hook_point_names_the_ones_that_exist() {
        let err = HookPoint::from_str("agent_started").unwrap_err();
        assert!(err.contains("agent_started"), "{err}");
        assert!(err.contains("agent_exit"), "{err}");
    }

    #[test]
    fn merging_replaces_a_bound_point_and_leaves_the_others() {
        let mut hooks = Hooks::default();
        let mut other = Hooks::from_data(BTreeMap::new());
        other.set(
            HookPoint::AgentExit,
            RunPluginOrAlias::from_url("file:/tmp/mine.lua", &None, None, None).unwrap(),
        );
        hooks.merge(other);
        assert_eq!(
            hooks.get(HookPoint::AgentExit).map(|p| p.location_string()),
            Some("file:/tmp/mine.lua".to_owned())
        );
    }

    #[test]
    fn clearing_a_point_unbinds_it() {
        let mut hooks = Hooks::default();
        hooks.clear(HookPoint::AgentExit);
        assert!(hooks.get(HookPoint::AgentExit).is_none());
        assert!(hooks.is_empty());
    }
}

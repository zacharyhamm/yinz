//! Hooks: named points where yinz hands control to a plugin.
//!
//! A hook point is a moment in the session's life that a plugin can own. The first one was the
//! moment an agent pane's command exits: before it existed, that moment was a branch in the
//! terminal emulator - `render_first_run_banner` painted a fixed question and
//! `TerminalPane::adjust_input_to_terminal` implemented its two answers - and nothing could be
//! added to it without editing the pane.
//!
//! A point has one of two [shapes](HookShape), and the shape is a fact about the point rather
//! than about the binding: `agent_exit` opens its plugin in place of the pane, every other point
//! hands the moment to a long-lived headless plugin as `Event::Hook`.
//!
//! A hook point takes **one** plugin, not a list. Two plugins cannot both replace the pane,
//! and a precedence rule between them would be a worse thing to explain than the restriction:
//! a hook that wants to do several things does them itself.
//!
//! ```kdl
//! hooks {
//!     agent_exit "file:my-agent-exit.scm"
//!     agent_start "file:my-policies.scm"
//! }
//! ```
//!
//! An empty string disables a hook point, leaving the unhooked behaviour in its place.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::layout::{RunPlugin, RunPluginLocation, RunPluginOrAlias};
use crate::data::PluginTag;

/// A moment a plugin can be given control of.
///
/// The KDL spelling is the snake_case name; `FromStr`/`Display` are the whole of the mapping,
/// so adding a point is adding a variant, its name and its [shape](HookPoint::shape). The serde
/// spelling is the same name, so a Scheme plugin reading the event point sees what it wrote in its
/// config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPoint {
    /// An agent pane's command exited and the pane is holding. The hook plugin is opened in
    /// place of that pane, which is suppressed behind it rather than closed.
    AgentExit,
    /// A pane gained an agent record: it was spawned as an agent, or the agent running in it
    /// reported for itself the first time (or again, after exiting). Where "spawn the next agent
    /// when this one starts" and "answer this prompt for it" policies live.
    AgentStart,
    /// An agent's record moved to `waiting`: it is blocked on a permission prompt or a question.
    AgentWaiting,
    /// An agent's record moved to `idle` from `busy` or `waiting`: its turn ended, and it is at
    /// its prompt with nothing asked of it. Where "read what it said and decide what is next"
    /// policies live.
    AgentIdle,
    /// Any command pane's command exited, held or not. `agent_exit` is the agent-only, pane-
    /// replacing counterpart; this one fires first, for every command pane, to a headless plugin.
    PaneExit,
    /// The session's first client connected: the plugin thread is up and its background
    /// plugins are loading. The first tab may not exist yet, so a hook that wants to open
    /// panes should wait for the first `tab_update`.
    SessionStart,
    /// A right-click asked for a context menu. The hook plugin is *not* opened at all: it runs
    /// headless, is handed the click as `Event::ContextMenuRequested`, and answers with
    /// `set_context_menu`. The menu itself is drawn by the server and is not a pane.
    ContextMenu,
    /// One shared headless policy decides notification delivery.
    Notification,
}

/// How the plugin bound to a point is run when the point is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookShape {
    /// The plugin is opened in place of the pane the hook is about, with the payload as its
    /// configuration - one instance per firing.
    ReplacePane,
    /// The plugin is long-lived and has no pane; the payload arrives as `Event::Hook` (or, for
    /// `context_menu`, `Event::ContextMenuRequested`) - one instance for every firing.
    Headless,
}

impl HookPoint {
    pub const ALL: &'static [HookPoint] = &[
        HookPoint::AgentExit,
        HookPoint::AgentStart,
        HookPoint::AgentWaiting,
        HookPoint::AgentIdle,
        HookPoint::PaneExit,
        HookPoint::SessionStart,
        HookPoint::ContextMenu,
        HookPoint::Notification,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            HookPoint::AgentExit => "agent_exit",
            HookPoint::AgentStart => "agent_start",
            HookPoint::AgentWaiting => "agent_waiting",
            HookPoint::AgentIdle => "agent_idle",
            HookPoint::PaneExit => "pane_exit",
            HookPoint::SessionStart => "session_start",
            HookPoint::ContextMenu => "context_menu",
            HookPoint::Notification => "notification",
        }
    }

    /// The shape is a property of the point, not of the firing: a pane-replacing point needs a
    /// pane the screen has already put in a state it can be replaced from, and a headless point
    /// may have no pane at all. Letting a fire site choose would allow both mistakes.
    pub fn shape(&self) -> HookShape {
        match self {
            HookPoint::AgentExit => HookShape::ReplacePane,
            HookPoint::AgentStart
            | HookPoint::AgentWaiting
            | HookPoint::AgentIdle
            | HookPoint::PaneExit
            | HookPoint::SessionStart
            | HookPoint::ContextMenu
            | HookPoint::Notification => HookShape::Headless,
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
            "agent_start" | "agent-start" => Ok(HookPoint::AgentStart),
            "agent_waiting" | "agent-waiting" => Ok(HookPoint::AgentWaiting),
            "agent_idle" | "agent-idle" => Ok(HookPoint::AgentIdle),
            "pane_exit" | "pane-exit" => Ok(HookPoint::PaneExit),
            "session_start" | "session-start" => Ok(HookPoint::SessionStart),
            "context_menu" | "context-menu" => Ok(HookPoint::ContextMenu),
            "notification" => Ok(HookPoint::Notification),
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
        {
            let plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
                location: RunPluginLocation::Yinz(PluginTag::new("agent-exit")),
                ..Default::default()
            });
            hooks.insert(HookPoint::AgentExit, plugin);
        }
        {
            let plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
                location: RunPluginLocation::Yinz(PluginTag::new("context-menu")),
                ..Default::default()
            });
            hooks.insert(HookPoint::ContextMenu, plugin);
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
#[path = "unit/hooks_tests.rs"]
mod tests;

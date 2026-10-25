//! Which plugin commands act on the calling plugin's own pane.
//!
//! A plugin that declares no pane runs headless: it has no pane record at all, so a command that
//! moves, hides, renames or replaces its own pane has nothing to act on. The host refuses those
//! commands with an error naming the missing declaration, rather than letting them disappear into
//! a screen that finds no such pane.
//!
//! Shared, so that the plugin host's gate, its mock `.test.scm` harness and the builtin plugins'
//! unit-test host all refuse exactly the same commands.

use crate::data::PluginCommand;
use crate::geometry::FloatingPaneCoordinates;
use crate::internal_ids::PluginId;
use crate::pane_id::PaneId;

/// The commands that always act on the calling plugin's own pane, whatever their arguments:
/// the names [`own_pane_command`] gives them. The others it names do so only when they target
/// the caller's pane by id.
pub const UNCONDITIONAL_OWN_PANE_COMMANDS: [&str; 8] = [
    "HideSelf",
    "ShowSelf",
    "SetSelectable",
    "ShowCursor",
    "SetSelfMouseSelectionSupport",
    "OpenTerminalInPlaceOfPlugin",
    "OpenFileInPlaceOfPlugin",
    "OpenCommandPaneInPlaceOfPlugin",
];

/// The error a headless plugin `plugin` gets for running the own-pane command `command`.
pub fn own_pane_refusal(command: &str, plugin: &str) -> String {
    format!(
        "'{command}' needs the plugin's own pane, but {plugin} declares no pane; add \
         (yinz-declare-pane) at the top level of its entry file"
    )
}

/// The name of `command` when it needs the pane of the plugin `me`, `None` when it does not.
///
/// Commands that address panes by id need the plugin's own pane only when one of their targets is
/// `PaneId::Plugin(me)`. `CloseSelf`, `ClosePluginPane(me)`, `InterceptKeyPresses`, `Notify` and
/// permission requests are deliberately absent: each of them means something for a headless
/// plugin too.
pub fn own_pane_command(command: &PluginCommand, me: PluginId) -> Option<&'static str> {
    let own = PaneId::Plugin(me);
    let targets_me = |panes: &[PaneId]| panes.contains(&own);
    let placements_target_me = |placements: &[(PaneId, FloatingPaneCoordinates)]| {
        placements.iter().any(|(pane_id, _)| *pane_id == own)
    };
    let name = match command {
        PluginCommand::HideSelf => "HideSelf",
        PluginCommand::ShowSelf(..) => "ShowSelf",
        PluginCommand::SetSelectable(..) => "SetSelectable",
        PluginCommand::ShowCursor(..) => "ShowCursor",
        PluginCommand::SetSelfMouseSelectionSupport(..) => "SetSelfMouseSelectionSupport",
        PluginCommand::OpenTerminalInPlaceOfPlugin(..) => "OpenTerminalInPlaceOfPlugin",
        PluginCommand::OpenFileInPlaceOfPlugin(..) => "OpenFileInPlaceOfPlugin",
        PluginCommand::OpenCommandPaneInPlaceOfPlugin(..) => "OpenCommandPaneInPlaceOfPlugin",
        PluginCommand::RenamePluginPane(plugin_id, _) if *plugin_id == me => "RenamePluginPane",
        PluginCommand::FocusPluginPane(plugin_id, ..) if *plugin_id == me => "FocusPluginPane",
        PluginCommand::HidePaneWithId(pane_id) if *pane_id == own => "HidePaneWithId",
        PluginCommand::ShowPaneWithId(pane_id, ..) if *pane_id == own => "ShowPaneWithId",
        PluginCommand::SetFloatingPanePinned(pane_id, _) if *pane_id == own => {
            "SetFloatingPanePinned"
        },
        PluginCommand::ChangeFloatingPanesCoordinates(placements)
            if placements_target_me(placements) =>
        {
            "ChangeFloatingPanesCoordinates"
        },
        PluginCommand::ChangeOverlayCoordinates { placements, .. }
            if placements_target_me(placements) =>
        {
            "ChangeOverlayCoordinates"
        },
        PluginCommand::BreakPanesToNewTab(panes, ..) if targets_me(panes) => "BreakPanesToNewTab",
        PluginCommand::BreakPanesToTabWithIndex(panes, ..) if targets_me(panes) => {
            "BreakPanesToTabWithIndex"
        },
        PluginCommand::BreakPanesToTabWithId(panes, ..) if targets_me(panes) => {
            "BreakPanesToTabWithId"
        },
        PluginCommand::SetPaneTitlePrefix { pane_id, .. } if *pane_id == own => {
            "SetPaneTitlePrefix"
        },
        PluginCommand::SetPaneSubtitle { pane_id, .. } if *pane_id == own => "SetPaneSubtitle",
        PluginCommand::HighlightAndUnhighlightPanes(highlight, unhighlight)
            if targets_me(highlight) || targets_me(unhighlight) =>
        {
            "HighlightAndUnhighlightPanes"
        },
        _ => return None,
    };
    Some(name)
}

#[cfg(test)]
#[path = "unit/own_pane.rs"]
mod tests;

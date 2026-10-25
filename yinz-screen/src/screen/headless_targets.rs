//! Instructions that name a headless plugin's pane. Such a plugin has no pane record, so every
//! handler that looks its id up in the tabs would find nothing and, for most of them, end the
//! action as if it had worked. They are settled here, before dispatch: a close by id unloads
//! the plugin, and anything else that needs the pane is refused with the reason, on its
//! completion when it has one and in the log when it does not.
use super::{Screen, ScreenInstruction};
use crate::panes::PaneId;
use crate::plugins::PluginId;
use crate::route::NotificationEnd;

impl Screen {
    /// Whether `pane_id` was a pane once and has since been closed, or names a plugin the
    /// plugin thread has unloaded: closing it again has nothing left to do.
    pub(super) fn pane_is_gone(&self, pane_id: PaneId) -> bool {
        self.destroyed_panes.contains(&pane_id)
            || matches!(pane_id, PaneId::Plugin(plugin_id) if self.unloaded_plugins.contains(&plugin_id))
    }

    /// The headless plugin whose pane `pane_id` names, if it names one. An id a tab still holds
    /// as a pane (a pane tabs container's blank slot) names that pane, not the plugin.
    fn headless_plugin(&self, pane_id: PaneId) -> Option<PluginId> {
        match pane_id {
            PaneId::Plugin(plugin_id)
                if self.paneless_plugins.contains_key(&plugin_id)
                    && !self.holds_pane_record(plugin_id) =>
            {
                Some(plugin_id)
            },
            _ => None,
        }
    }

    fn first_headless_plugin(
        &self,
        pane_ids: impl IntoIterator<Item = PaneId>,
    ) -> Option<PluginId> {
        pane_ids
            .into_iter()
            .find_map(|pane_id| self.headless_plugin(pane_id))
    }

    /// Answers `completion` with why `plugin_id` has no pane to act on.
    fn refuse_for_headless(&self, plugin_id: PluginId, completion: Option<NotificationEnd>) {
        let error = self
            .headless_plugin_error(plugin_id)
            .unwrap_or_else(|| format!("plugin {plugin_id} has no pane"));
        log::warn!("{error}");
        if let Some(mut completion) = completion {
            completion.set_exit_status(1);
            completion.set_error_message(error);
        }
    }

    /// Settles `event` when it names a headless plugin's pane, returning `None`; any other
    /// instruction comes back for dispatch. `FocusPaneWithId` answers for itself in
    /// `focus_pane_by_request`, and `ClosePane` in `close_pane_from_instruction`, which also
    /// sees the closes replayed for a pane that was still being placed.
    pub(super) fn settle_headless_target(
        &mut self,
        event: ScreenInstruction,
    ) -> Option<ScreenInstruction> {
        use ScreenInstruction as I;
        match event {
            I::CloseFocusWithPaneId(pane_id, completion) => match pane_id {
                // Closing a headless plugin, or one still resolving its repository, unloads it;
                // the plugin thread reports the unload with `PluginUnloaded`, and the action is
                // done once it is asked for.
                PaneId::Plugin(plugin_id) if self.closes_by_unloading(plugin_id) => {
                    self.unload_paneless_plugin(plugin_id);
                    drop(completion);
                    None
                },
                _ => Some(I::CloseFocusWithPaneId(pane_id, completion)),
            },
            // One pane, and a completion to answer.
            I::RenamePane(pane_id, _, completion)
            | I::RenamePaneWithPaneId(pane_id, _, completion)
            | I::UndoRenamePaneWithPaneId(pane_id, completion)
            | I::TogglePanePinnedWithPaneId(pane_id, completion)
            | I::TogglePaneEmbedOrFloatingWithPaneId(pane_id, completion)
            | I::ToggleFullscreenWithPaneId(pane_id, completion)
            | I::ToggleNoUiFullscreenWithPaneId(pane_id, completion)
            | I::ScrollUpWithPaneId(pane_id, completion)
            | I::ScrollDownWithPaneId(pane_id, completion)
            | I::ScrollToTopWithPaneId(pane_id, completion)
            | I::ScrollToBottomWithPaneId(pane_id, completion)
            | I::PageScrollUpWithPaneId(pane_id, completion)
            | I::PageScrollDownWithPaneId(pane_id, completion)
            | I::HalfPageScrollUpWithPaneId(pane_id, completion)
            | I::HalfPageScrollDownWithPaneId(pane_id, completion)
            | I::ResizeWithPaneId(pane_id, _, completion)
            | I::MovePaneWithPaneIdCli(pane_id, _, completion)
            | I::MovePaneBackwardsWithPaneId(pane_id, completion)
            | I::ClearScreenWithPaneId(pane_id, completion)
            | I::EditScrollbackWithPaneId(pane_id, _, completion)
            | I::EditScrollbackForPaneWithId(pane_id, completion)
            | I::TogglePaneBorderless(pane_id, completion)
            | I::SetPaneBorderless(pane_id, _, completion)
            | I::WriteToPaneId(_, pane_id, completion)
            | I::WriteKeyToPaneId(_, _, _, pane_id, completion)
            | I::Paste(_, Some(pane_id), _, completion)
            | I::SetPaneColor(pane_id, _, _, completion)
            // These two park for a pane that is still being placed; a headless plugin's never
            // will be, so they would wait until it unloads.
            | I::SetPaneAgent(pane_id, _, completion)
            | I::SetPaneAgentRun(pane_id, _, completion)
                if let Some(plugin_id) = self.headless_plugin(pane_id) =>
            {
                self.refuse_for_headless(plugin_id, completion);
                None
            },
            I::ReplacePaneWithExistingPane(replaced, replacement, _, completion)
                if let Some(plugin_id) = self.first_headless_plugin([replaced, replacement]) =>
            {
                self.refuse_for_headless(plugin_id, completion);
                None
            },
            // Several panes, one completion: the action is refused as a whole rather than done
            // for some of the panes it names.
            I::StackPanes(pane_ids, _, completion)
            | I::BreakPanesToNewTab {
                pane_ids,
                completion_tx: completion,
                ..
            }
            | I::BreakPanesToTabWithIndex {
                pane_ids,
                completion_tx: completion,
                ..
            }
            | I::BreakPanesToTabWithId {
                pane_ids,
                completion_tx: completion,
                ..
            } if let Some(plugin_id) = self.first_headless_plugin(pane_ids.iter().copied()) => {
                self.refuse_for_headless(plugin_id, completion);
                None
            },
            I::ChangeFloatingPanesCoordinates(placements, completion)
            | I::ChangeOverlayCoordinates(placements, _, completion)
                if let Some(plugin_id) =
                    self.first_headless_plugin(placements.iter().map(|(pane_id, _)| *pane_id)) =>
            {
                self.refuse_for_headless(plugin_id, completion);
                None
            },
            // One pane and no completion: plugin commands, which a plugin sends for another
            // plugin's pane by id. Only the log can say why nothing happened.
            I::SuppressPane(pane_id, _)
            | I::UnsuppressPane(pane_id, _)
            | I::UnsuppressOrExpandPane(pane_id, _)
            | I::SetFloatingPanePinned(pane_id, _)
            | I::MovePaneWithPaneId(pane_id)
            | I::MovePaneWithPaneIdInDirection(pane_id, _)
            | I::ResizePaneWithId(_, pane_id)
            | I::TogglePaneIdFullscreen(pane_id)
            | I::TogglePaneEmbedOrEjectForPaneId(pane_id)
            | I::ClearScreenForPaneId(pane_id)
            | I::ScrollUpInPaneId(pane_id)
            | I::ScrollDownInPaneId(pane_id)
            | I::ScrollToTopInPaneId(pane_id)
            | I::ScrollToBottomInPaneId(pane_id)
            | I::PageScrollUpInPaneId(pane_id)
            | I::PageScrollDownInPaneId(pane_id)
            | I::RevealPaneWithId(pane_id, _, _)
                if let Some(plugin_id) = self.headless_plugin(pane_id) =>
            {
                self.refuse_for_headless(plugin_id, None);
                None
            },
            // A plugin's own selectability: meaningless without a pane, and parked for a tab
            // that would never hold it.
            I::SetSelectable(pane_id, _) if self.headless_plugin(pane_id).is_some() => None,
            // Several panes and no completion: the headless ones are left out, the rest moved.
            I::FloatMultiplePanes(pane_ids, client_id) => Some(I::FloatMultiplePanes(
                self.without_headless(pane_ids),
                client_id,
            )),
            I::EmbedMultiplePanes(pane_ids, client_id) => Some(I::EmbedMultiplePanes(
                self.without_headless(pane_ids),
                client_id,
            )),
            event => Some(event),
        }
    }

    fn without_headless(&self, mut pane_ids: Vec<PaneId>) -> Vec<PaneId> {
        pane_ids.retain(|pane_id| match self.headless_plugin(*pane_id) {
            Some(plugin_id) => {
                self.refuse_for_headless(plugin_id, None);
                false
            },
            None => true,
        });
        pane_ids
    }
}

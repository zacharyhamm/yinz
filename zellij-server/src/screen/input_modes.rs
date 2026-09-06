use zellij_runtime::errors::prelude::*;

use super::{active_tab, Screen};
use crate::panes::PaneId;
use crate::plugins::{PluginId, PluginInstruction};
use crate::tab::Tab;
use crate::{ClientId, ServerInstruction};
use zellij_utils::data::{Event, EventType, InputMode, ModeInfo};

impl Screen {
    pub fn change_mode(
        &mut self,
        new_mode: InputMode,
        base_mode: Option<InputMode>,
        client_id: ClientId,
    ) -> Result<()> {
        let mut mode_info = self
            .mode_info
            .get(&client_id)
            .cloned()
            .unwrap_or_else(|| self.default_mode_info.clone());
        let previous_mode = mode_info.mode;
        mode_info.mode = new_mode;
        mode_info.base_mode = base_mode;
        if mode_info.session_name.as_ref() != Some(&self.session_name) {
            mode_info.session_name = Some(self.session_name.clone());
        }

        if previous_mode != InputMode::Select && new_mode == InputMode::Select {
            active_tab!(self, client_id, |tab: &mut Tab| tab
                .start_text_selection(client_id));
        } else if previous_mode == InputMode::Select && new_mode != InputMode::Select {
            // every tab, not just the active one: the selection is finished on the tab it was
            // started on even if focus moved to another tab in the meantime (a tab with no
            // keyboard selection for this client is a no-op)
            for tab in self.tabs.values_mut() {
                tab.finish_text_selection(client_id);
            }
        }

        // If we leave the Search-related modes, we need to clear all previous searches
        let search_related_modes = [InputMode::EnterSearch, InputMode::Search, InputMode::Scroll];
        if search_related_modes.contains(&previous_mode)
            && !search_related_modes.contains(&mode_info.mode)
        {
            active_tab!(self, client_id, |tab: &mut Tab| tab.clear_search(client_id));
        }

        if mode_info.mode == InputMode::RenameTab {
            if let Ok(active_tab) = self.get_active_tab_mut(client_id) {
                active_tab.prev_name = active_tab.name.clone();
            }
        }

        if mode_info.mode == InputMode::RenamePane {
            if let Ok(active_tab) = self.get_active_tab_mut(client_id) {
                if let Some(active_pane) =
                    active_tab.get_active_pane_or_floating_pane_mut(client_id)
                {
                    active_pane.store_pane_name();
                }
            }
        }

        self.style = mode_info.style;
        self.mode_info.insert(client_id, mode_info.clone());
        for tab in self.tabs.values_mut() {
            tab.change_mode_info(mode_info.clone(), client_id);
            tab.mark_active_pane_for_rerender(client_id);
            tab.update_input_modes()?;
        }
        // Notify background plugins subscribed to ModeUpdate
        let mut bg_updates = vec![];
        for ((bg_pid, bg_cid), subs) in &self.background_plugin_subscriptions {
            if subs.contains(&EventType::ModeUpdate) && *bg_cid == client_id {
                bg_updates.push((
                    Some(*bg_pid),
                    Some(*bg_cid),
                    Event::ModeUpdate(mode_info.clone()),
                ));
            }
        }
        if !bg_updates.is_empty() {
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::Update(bg_updates))
                .context("failed to update background plugins with mode info")?;
        }
        Ok(())
    }
    // Keep the client's mode in sync with the pane it just focused: entering a scrolled pane
    // switches to Scroll so its position is navigable, leaving it
    // returns to the default mode. Only the default<->Scroll pair is touched (Normal by
    // default, Locked under unlock-first), so a client in another mode (Pane, Tab, Search,
    // ...) is never pulled out of it.
    pub(super) fn sync_scroll_mode_on_focus(&mut self, client_id: ClientId) -> Result<()> {
        let default_mode = self.base_input_mode();
        if default_mode == InputMode::Scroll {
            return Ok(());
        }
        let current_mode = match self.mode_info.get(&client_id) {
            Some(mode_info) => mode_info.mode,
            None => return Ok(()),
        };
        let active_pane_is_scrolled = self.active_pane_is_scrolled(client_id);
        let new_mode = match (current_mode, active_pane_is_scrolled) {
            (mode, true) if mode == default_mode => InputMode::Scroll,
            (InputMode::Scroll, false) => default_mode,
            _ => return Ok(()),
        };
        // Route through the server like SwitchToMode does, so the client's authoritative
        // input mode (current_input_modes, used for keybind resolution) is updated and not
        // just the mode_info used for rendering. A direct change_mode() would desync the two:
        // the status line would show Scroll while keys still resolved in the old mode.
        self.bus
            .senders
            .send_to_server(ServerInstruction::ChangeMode(client_id, new_mode, None))
            .with_context(|| format!("failed to sync scroll mode on focus for client {client_id}"))
    }
    // Whether the client's currently focused pane is scrolled.
    pub(super) fn active_pane_is_scrolled(&mut self, client_id: ClientId) -> bool {
        self.get_active_tab(client_id)
            .map(|tab| tab.active_pane_is_scrolled(client_id))
            .unwrap_or(false)
    }
    pub(super) fn sync_scroll_mode_if_scroll_changed(
        &mut self,
        client_id: ClientId,
        was_scrolled: bool,
    ) -> Result<()> {
        if self.active_pane_is_scrolled(client_id) == was_scrolled {
            return Ok(());
        }
        self.sync_scroll_mode_on_focus(client_id)
    }
    // Sync the scroll mode for every connected client currently focused on `pane_id`, used when
    // a pane's scroll position changed by pane id rather than by the acting client's own focus
    // (e.g. ScrollUpInPaneId, used by plugins).
    pub(super) fn sync_scroll_mode_for_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        let client_ids: Vec<ClientId> = self.active_tab_ids.keys().copied().collect();
        for client_id in client_ids {
            if self.active_pane_id(client_id) == Some(pane_id) {
                self.sync_scroll_mode_on_focus(client_id)?;
            }
        }
        Ok(())
    }
    pub fn change_mode_for_all_clients(
        &mut self,
        new_mode: InputMode,
        base_mode: Option<InputMode>,
    ) -> Result<()> {
        let err_context = || {
            format!(
                "failed to change input mode to {:?} for all clients",
                new_mode
            )
        };

        let connected_client_ids: Vec<ClientId> = self.active_tab_ids.keys().copied().collect();
        for client_id in connected_client_ids {
            self.change_mode(new_mode, base_mode, client_id)
                .with_context(err_context)?;
        }
        Ok(())
    }
    /// Collect plugin IDs that should receive a broadcast event for a given client.
    /// Returns plugin IDs from the client's active tab plus background plugins
    /// subscribed to the given event type.
    pub(super) fn targeted_plugin_ids(
        &self,
        client_id: ClientId,
        event_type: EventType,
    ) -> Vec<PluginId> {
        let mut plugin_ids = Vec::new();
        // Active-tab plugins
        if let Some(active_tab_id) = self.active_tab_ids.get(&client_id) {
            if let Some(tab) = self.tabs.get(active_tab_id) {
                plugin_ids.extend(tab.get_plugin_ids());
            }
        }
        // An overlay follows the client between tabs under its own steam - hocus moves itself on
        // `TabUpdate` - so a client's active tab is not necessarily where the overlay is right
        // now, and the active-tab walk above would leave it deaf to the very event it moves on.
        //
        // **The two state events only.** Those two are what an
        // overlay acts on from a tab nobody is looking at: `TabUpdate` is what it moves on, and
        // `PaneUpdate` is how it learns it was hidden or revealed. An input event is one client's
        // keystroke on one tab, and handing it to an overlay sitting somewhere else would have it
        // clear or otherwise mutate its UI on behalf of input the user never sent it - a
        // background overlay subscribed to `InputReceived` would hear every other tab's typing.
        // An overlay on the client's *active* tab is untouched by this: `Tab::get_plugin_ids`
        // already walks `overlay_panes`, hidden ones included, so it keeps the ordinary routing
        // for every event type.
        if matches!(event_type, EventType::TabUpdate | EventType::PaneUpdate) {
            for tab in self.tabs.values() {
                for plugin_id in tab.overlay_plugin_ids() {
                    if !plugin_ids.contains(&plugin_id) {
                        plugin_ids.push(plugin_id);
                    }
                }
            }
        }
        // Background plugins subscribed to this event type
        for ((bg_pid, bg_cid), subs) in &self.background_plugin_subscriptions {
            if subs.contains(&event_type) && *bg_cid == client_id {
                if !plugin_ids.contains(bg_pid) {
                    plugin_ids.push(*bg_pid);
                }
            }
        }
        plugin_ids
    }
    /// Broadcast a ModeUpdate event to active-tab plugins and subscribed background plugins.
    pub fn broadcast_mode_update(
        &mut self,
        mode_info: ModeInfo,
        target_client_id: Option<ClientId>,
    ) -> Result<()> {
        let mut plugin_updates = vec![];
        let client_ids: Vec<ClientId> = if let Some(cid) = target_client_id {
            vec![cid]
        } else {
            self.active_tab_ids.keys().copied().collect()
        };
        for client_id in client_ids {
            let plugin_ids = self.targeted_plugin_ids(client_id, EventType::ModeUpdate);
            for plugin_id in plugin_ids {
                plugin_updates.push((
                    Some(plugin_id),
                    Some(client_id),
                    Event::ModeUpdate(mode_info.clone()),
                ));
            }
        }
        if !plugin_updates.is_empty() {
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::Update(plugin_updates))
                .context("failed to broadcast mode update")?;
        }
        Ok(())
    }
}

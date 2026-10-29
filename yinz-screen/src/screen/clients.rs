//! Client registration, following and watcher lifecycle.
use super::Screen;
use super::WatcherState;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::ClientId;
use crate::ServerInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;
use yinz_types::pane_size::Size;

impl Screen {
    #[cfg(test)]
    pub fn add_client(&mut self, client_id: ClientId) -> Result<()> {
        let departed = self.departed_follow_source();
        self.register_client_transaction(client_id, departed, None, true)
            .map(|_| ())
    }

    /// Register before sending client-specific plugin updates, then commit only after reflow.
    /// Returns the departed source whose queued plugin output must be discarded before replay.
    pub(super) fn register_client_transaction(
        &mut self,
        client_id: ClientId,
        departed: Option<(ClientId, Option<TabId>)>,
        client_size: Option<Size>,
        attaching: bool,
    ) -> Result<Option<ClientId>> {
        let result = (|| {
            if let Some(size) = client_size {
                self.set_client_size(client_id, size)
                    .context("setting the registering client's size")?;
            }
            if attaching {
                self.bus
                    .senders
                    .send_to_plugin(PluginInstruction::AddClient(client_id))
                    .context("adding the registering client to the plugins")?;
            }
            self.register_client(client_id)
                .context("registering the client with the screen")?;
            if client_size.is_some() {
                // Attachment can shrink the shared surface; reflow every tab before committing.
                self.reflow_all_tabs()
                    .context("reflowing tabs for the attaching client")?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                let retired_source = self.commit_following_client(client_id, departed);
                // Shared plugins can retain state for a recycled client id. Send even unknown
                // geometry, but only after plugin registration and the screen's attach commit.
                self.report_cell_pixel_size(None, client_id);
                Ok(retired_source)
            },
            Err(error) => {
                self.rollback_client_registration(client_id, departed);
                if client_size.is_some() {
                    self.resize_to_clients().non_fatal();
                }
                if attaching {
                    self.bus
                        .senders
                        .send_to_server(ServerInstruction::ClientExit(client_id, None))
                        .context("disconnecting client after failed registration")
                        .non_fatal();
                }
                Err(error)
            },
        }
    }

    pub(super) fn departed_follow_source(&self) -> Option<(ClientId, Option<TabId>)> {
        self.followed_client_id
            .filter(|id| !self.connected_clients.borrow().contains(id))
            .map(|id| (id, self.departed_followed_tab_id))
    }

    pub(super) fn rollback_client_registration(
        &mut self,
        client_id: ClientId,
        departed: Option<(ClientId, Option<TabId>)>,
    ) {
        // A reused source id is provisional: removing it must preserve its retained pane state.
        self.connected_clients.borrow_mut().remove(&client_id);
        self.active_tab_ids.remove(&client_id);
        self.tab_history.remove(&client_id);
        self.client_sizes.remove(&client_id);
        self.client_pixel_dimensions.remove(&client_id);
        self.client_cell_pixel_sizes.remove(&client_id);
        for tab in self.tabs.values_mut() {
            tab.remove_client(client_id);
        }
        self.remove_client_inner(client_id, true).non_fatal();
        if let Some((source, tab)) = departed {
            self.followed_client_id = Some(source);
            self.departed_followed_tab_id = tab;
        }
    }

    pub(super) fn commit_following_client(
        &mut self,
        client_id: ClientId,
        departed: Option<(ClientId, Option<TabId>)>,
    ) -> Option<ClientId> {
        if self.watcher_clients.contains_key(&client_id)
            || (departed.is_none() && self.followed_client_id.is_some())
        {
            return None;
        }
        if let Some((source, _)) = departed.filter(|(source, _)| *source != client_id) {
            for tab in self.tabs.values_mut() {
                tab.remove_client_pane_state(source);
            }
        }
        self.followed_client_id = Some(client_id);
        self.departed_followed_tab_id = None;
        self.pane_subscriber_snapshots_dirty = true;
        for watcher in self.watcher_clients.values_mut() {
            watcher.set_force_render();
        }
        departed.map(|(source, _)| source)
    }

    pub(super) fn register_client(&mut self, client_id: ClientId) -> Result<()> {
        let attach_started_at = std::time::Instant::now();
        let err_context = |tab_index| {
            format!("failed to attach client {client_id} to tab with index {tab_index}")
        };
        log::info!("attach trace: Screen::add_client client_id={client_id} state setup begin");

        let mut tab_history = vec![];
        if let Some((_first_client, first_tab_history)) = self.tab_history.iter().next() {
            tab_history = first_tab_history.clone();
        }

        // Checked against the tabs there are, like the three fallbacks below it: an entry naming
        // a tab that has gone is exactly what a client cannot recover from, and inheriting one
        // would hand the arriving client the same dead end.
        let tab_index = if let Some((_first_client, first_active_tab_index)) = self
            .active_tab_ids
            .iter()
            .find(|(_, tab_id)| self.tabs.contains_key(tab_id))
        {
            *first_active_tab_index
        } else if self.tabs.contains_key(&self.global_last_active_tab_id) {
            self.global_last_active_tab_id
        } else if self.tabs.contains_key(&TabId::new(0)) {
            TabId::new(0)
        } else if let Some(tab_index) = self.tabs.keys().next() {
            tab_index.to_owned()
        } else {
            bail!("Can't find a valid tab to attach client to!");
        };

        self.active_tab_ids.insert(client_id, tab_index);
        // a reattaching client id inherits a blank terminal, not the fill the last one was sent
        self.unowned_strip_fill.remove(&client_id);
        self.last_notification_fill.remove(&client_id);
        self.client_kitty_host_state.borrow_mut().remove(&client_id);
        self.connected_clients.borrow_mut().insert(client_id);
        self.show_toasts_held_for_the_first_client()
            .with_context(|| err_context(tab_index))?;
        self.tab_history.insert(client_id, tab_history);
        let tab = self
            .tabs
            .get_mut(&tab_index)
            .with_context(|| err_context(tab_index))?;
        // `remove_client` tells a tab's plugins nobody is looking (`visible(false)`) once
        // the last client leaves; a client arriving in an empty tab is the other half.
        let tab_was_empty = tab.has_no_connected_clients();
        log::info!(
            "attach trace: Screen::add_client client_id={client_id} tab={tab_index} Tab::add_client begin empty={tab_was_empty}"
        );
        // Startup probes can populate ModeInfo before the first layout finishes loading.
        tab.add_client(client_id, self.mode_info.get(&client_id).cloned())
            .with_context(|| err_context(tab_index))?;
        log::info!(
            "attach trace: Screen::add_client client_id={client_id} tab={tab_index} Tab::add_client complete after {:?}",
            attach_started_at.elapsed()
        );
        if tab_was_empty {
            log::info!(
                "attach trace: Screen::add_client client_id={client_id} tab={tab_index} visible(true) begin"
            );
            tab.visible(true).with_context(|| err_context(tab_index))?;
            log::info!(
                "attach trace: Screen::add_client client_id={client_id} tab={tab_index} visible(true) complete after {:?}",
                attach_started_at.elapsed()
            );
        }
        log::info!(
            "attach trace: Screen::add_client client_id={client_id} tab={tab_index} recompute_tab_size begin"
        );
        self.recompute_tab_size(tab_index)
            .with_context(|| err_context(tab_index))?;
        if !self.nested_ancestry.is_empty() || !self.host_descend_keys.is_empty() {
            self.update_all_clients_nesting_mode_info();
        }
        log::info!(
            "attach trace: Screen::add_client client_id={client_id} complete after {:?}",
            attach_started_at.elapsed()
        );
        Ok(())
    }

    pub fn remove_client(&mut self, client_id: ClientId) -> Result<()> {
        let was_attached = self.is_client_attached(client_id);
        if self.has_pending_tabs() {
            self.departed_before_layout.insert(client_id);
        }
        self.remove_client_inner(client_id, was_attached)
    }

    pub(super) fn remove_client_inner(
        &mut self,
        client_id: ClientId,
        clean_panes: bool,
    ) -> Result<()> {
        let err_context = || format!("failed to remove client {client_id}");
        // `and` evaluates every cleanup step while retaining the first error.
        let mut result = Ok(());

        self.notification_mouse_captures.remove(&client_id);
        self.close_notification_browser(client_id);
        // No force-render: the client is going, and the cells its menu occupied belong to a screen
        // nobody is looking at any more.
        self.context_menus.remove(&client_id);
        self.close_command_palette(client_id);
        self.disconnect_palette_client(client_id);
        self.command_palette_mouse_captures.remove(&client_id);
        self.client_projects.remove(&client_id);
        self.set_client_dimmed(client_id, false, None);
        let passthrough_panes: Vec<PaneId> = self
            .nested_guest_choices
            .iter()
            .filter(|((choice_client_id, _), choice)| {
                *choice_client_id == client_id && choice.enters_passthrough()
            })
            .map(|((_, choice_pane_id), _)| *choice_pane_id)
            .collect();
        for pane_id in passthrough_panes {
            let _ = self
                .bus
                .senders
                .send_to_server(ServerInstruction::KeyPassthroughChanged(
                    client_id, pane_id, pane_id, false, None, true,
                ));
        }
        self.nested_guest_choices
            .retain(|(choice_client_id, _), _| *choice_client_id != client_id);
        if clean_panes {
            for tab in self.tabs.values_mut() {
                tab.clear_guest_modal_for_client_on_all_panes(client_id);
                tab.clear_guest_choice_indicator_for_client_on_all_panes(client_id);
            }
        }

        // If the followed client disconnected, find the next regular client
        if Some(client_id) == self.followed_client_id
            && self.connected_clients.borrow().contains(&client_id)
        {
            // Try to find another regular (non-watcher) client
            let next_client = self
                .connected_clients
                .borrow()
                .iter()
                .copied()
                .find(|id| !self.watcher_clients.contains_key(id) && id != &client_id);

            // If no regular client remains but we have watchers, keep the old followed_client_id
            // and its tab as the watchers' render source.
            if let Some(next_client) = next_client {
                result = result.and(
                    self.set_followed_client(next_client)
                        .with_context(err_context),
                );
            } else if self.watcher_clients.is_empty() {
                self.followed_client_id = None;
                self.departed_followed_tab_id = None;
            } else {
                self.departed_followed_tab_id = self.active_tab_ids.get(&client_id).copied();
            }
        }

        if clean_panes {
            // Watchers still following this client keep seeing its pane state.
            let still_followed = self.followed_client_id == Some(client_id);
            for tab in self.tabs.values_mut() {
                tab.remember_departing_focus(client_id);
                if !still_followed {
                    tab.remove_client_pane_state(client_id);
                }
                tab.remove_client(client_id);
                if tab.has_no_connected_clients() {
                    result = result.and(tab.visible(false).with_context(err_context));
                }
            }
        }

        let previously_active_tab_id = self.active_tab_ids.get(&client_id).copied();
        if let Some(prev_tab_id) = previously_active_tab_id {
            self.global_last_active_tab_id = prev_tab_id;
            self.active_tab_ids.remove(&client_id);
        }
        self.tab_history.remove(&client_id);
        self.pane_history.remove(&client_id);
        self.permission_mouse_captures.remove(&client_id);
        self.current_pane_group
            .borrow_mut()
            .remove_client(&client_id);
        self.currently_marking_pane_group
            .borrow_mut()
            .remove(&client_id);
        // Plugin subscriptions stay: they belong to a plugin instance, which can outlive the
        // connection that owns it, and go when it is disposed or moves to another owner.
        self.connected_clients.borrow_mut().remove(&client_id);
        self.client_kitty_host_state.borrow_mut().remove(&client_id);
        self.kitty_probe_answers.remove(&client_id);
        let removed_kitty_capability = self
            .kitty_host_capabilities
            .borrow_mut()
            .remove(&client_id)
            .is_some();
        if removed_kitty_capability {
            self.push_kitty_host_support_to_tabs();
        }
        // No aggregate to push: this one is read per client at render time.
        self.synchronized_output_capabilities
            .borrow_mut()
            .remove(&client_id);
        let removed_sixel_capability = self
            .sixel_host_capabilities
            .borrow_mut()
            .remove(&client_id)
            .is_some();
        if removed_sixel_capability {
            self.push_sixel_host_support_to_tabs();
        }
        self.client_sizes.remove(&client_id);
        self.client_pixel_dimensions.remove(&client_id);
        self.client_cell_pixel_sizes.remove(&client_id);
        self.unowned_strip_fill.remove(&client_id);
        self.last_notification_fill.remove(&client_id);
        self.pane_render_subscribers.remove(&client_id);
        self.client_host_focused.remove(&client_id);
        self.client_notification_protocols.remove(&client_id);
        self.client_host_terminal_env.remove(&client_id);
        self.client_timezones.remove(&client_id);
        self.last_forwarded_osc7.remove(&client_id);
        // This entry describes the terminal on the other end: its probed
        // `capabilities.kitty_graphics`, its theme, its keybinds. Left behind, a departed client
        // becomes a phantom in every tab created afterwards, since `new_tab` copies this whole map
        // into each new one.
        self.mode_info.remove(&client_id);
        if let Some(prev_tab_id) = previously_active_tab_id {
            result = result.and(
                self.recompute_tab_size(prev_tab_id)
                    .with_context(err_context),
            );
        }
        if self.nested_via_client_id == Some(client_id) {
            self.nested_via_client_id = None;
            self.nested_ancestry = vec![];
            self.host_descended = false;
            self.host_descend_keys = vec![];
            self.update_all_clients_nesting_mode_info();
            self.broadcast_ancestry_update_to_guests();
        }
        // Deliberately no session-state report here. The `RemoveClient` handler
        // reports once, through `resize_to_clients`, and only for a client that
        // was actually attached — reporting here as well made every departing
        // connection cost the session two extra full-manifest broadcasts.
        if clean_panes {
            self.pane_subscriber_snapshots_dirty = true;
            // After the tabs' last `UpdateActivePane` for this client, on the same channel.
            self.bus
                .senders
                .send_to_pty(PtyInstruction::RemoveClient(client_id))
                .with_context(err_context)
                .non_fatal();
        }
        self.resume_deferred_render();
        result
    }

    /// Whether this client id belongs to a connection the screen actually knows —
    /// an attached client or a watcher. A session probe or a one-shot CLI action
    /// client connects and disconnects without ever attaching, and none of the
    /// screen's per-client bookkeeping applies to it.
    pub fn is_client_attached(&self, client_id: ClientId) -> bool {
        self.active_tab_ids.contains_key(&client_id)
            || self.watcher_clients.contains_key(&client_id)
    }

    pub fn add_watcher_client(&mut self, client_id: ClientId) -> Result<()> {
        // Initialize with a default size - will be updated when we receive the actual size
        let default_size = Size { rows: 24, cols: 80 }; // Reasonable default
        self.watcher_clients
            .insert(client_id, WatcherState::new(default_size));

        // Force a full render for the new watcher
        // This ensures they get complete state, not just delta
        self.render()?;

        Ok(())
    }

    pub fn remove_watcher_client(&mut self, client_id: ClientId) {
        self.watcher_clients.remove(&client_id);
        self.client_pixel_dimensions.remove(&client_id);
        self.client_cell_pixel_sizes.remove(&client_id);
        if self.watcher_clients.is_empty() && self.departed_follow_source().is_some() {
            self.clear_disconnected_followed_client_state();
            self.followed_client_id = None;
            self.departed_followed_tab_id = None;
        }
        self.resume_deferred_render();
        // A watcher reports its own synchronised-output support like any other client, and ids
        // are recycled - so its answer goes with it, as `remove_client` does for a regular one.
        self.synchronized_output_capabilities
            .borrow_mut()
            .remove(&client_id);
    }

    pub fn set_followed_client(&mut self, client_id: ClientId) -> Result<()> {
        if self.followed_client_id != Some(client_id) {
            self.clear_disconnected_followed_client_state();
            self.departed_followed_tab_id = None;
        }
        self.followed_client_id = Some(client_id);
        for watcher in self.watcher_clients.values_mut() {
            watcher.set_force_render();
        }
        // Trigger re-render with new followed client
        if !self.watcher_clients.is_empty() {
            self.render()?;
        }
        Ok(())
    }

    pub(super) fn clear_disconnected_followed_client_state(&mut self) {
        if let Some(client_id) = self
            .followed_client_id
            .filter(|id| !self.connected_clients.borrow().contains(id))
        {
            for tab in self.tabs.values_mut() {
                tab.remove_client_pane_state(client_id);
            }
            self.pane_subscriber_snapshots_dirty = true;
        }
    }

    pub fn set_watcher_size(&mut self, client_id: ClientId, size: Size) {
        // Update size if this client is a watcher
        if let Some(watcher_state) = self.watcher_clients.get_mut(&client_id) {
            watcher_state.set_size(size);
            watcher_state.set_force_render();
        }
    }
}

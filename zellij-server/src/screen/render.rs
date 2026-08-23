use zellij_utils::errors::prelude::*;

use std::collections::{BTreeMap, HashMap};
use zellij_utils::pane_size::{Size, Viewport};
use crate::{ClientId, ServerInstruction};
use crate::background_jobs::BackgroundJob;
use crate::output::Output;
use crate::panes::PaneId;
use crate::plugins::{PluginInstruction, PluginRenderAsset};
use super::Screen;

impl Screen {
    pub fn render(&mut self, plugin_render_assets: Option<Vec<PluginRenderAsset>>) -> Result<()> {
        // here we schedule the RenderToClients background job which debounces renders every 10ms
        // rather than actually rendering
        //
        // when this job decides to render, it sends back the ScreenInstruction::RenderToClients
        // message, triggering our render_to_clients method which does the actual rendering

        self.sync_nested_guest_fullscreen_state();

        let _ = self
            .bus
            .senders
            .send_to_background_jobs(BackgroundJob::RenderToClients);
        if let Some(plugin_render_assets) = plugin_render_assets {
            let _ = self
                .bus
                .senders
                .send_to_plugin(PluginInstruction::UnblockCliPipes(plugin_render_assets))
                .context("failed to unblock input pipe");
        }
        Ok(())
    }

    pub fn render_to_clients(&mut self) -> Result<()> {
        // this method does the actual rendering and is triggered by a debounced BackgroundJob (see
        // the render method for more details)
        let err_context = "failed to render screen";

        // Global panes and every tab share the same session surface. Re-derive each tab's center
        // from that surface so a render tick cannot restore stale geometry.
        let surface = self.surface_size();
        // A surface the global panes cannot be laid out over is a frame to drop, not a session to
        // end. This runs on a debounced background tick, so it sees whatever transient geometry the
        // instruction stream happens to be between - and `?` here reaches `screen_thread_main`'s
        // `.fatal()`, taking the whole session down over one unpaintable frame. The next tick
        // repaints from the state that follows.
        if let Err(error) = self.resize_global_panes_to(surface) {
            Err::<(), _>(error).context(err_context).non_fatal();
            return Ok(());
        }
        for tab_id in self.tabs.keys().copied().collect::<Vec<_>>() {
            // With no contributing client, fall back to the last known shared surface.
            let tab_size = self.tab_surface_size(tab_id).unwrap_or(surface);
            let layout = match self.global_layout_for(tab_size) {
                Ok(layout) => layout,
                // as above, but only this tab is unpaintable - the others still get their frame
                Err(error) => {
                    Err::<(), _>(error).context(err_context).non_fatal();
                    continue;
                },
            };
            let center = layout.center;
            let center_size = Size {
                cols: center.cols,
                rows: center.rows,
            };
            let strips = self.strips_from_layout(&layout);
            let borders = {
                let client_ids: Vec<ClientId> =
                    self.connected_clients.borrow().keys().copied().collect();
                let multiple_users_exist_in_session = client_ids.len() > 1;
                self.global_panes.tab_facing_borders(
                    &layout,
                    &client_ids,
                    &self.mode_info,
                    &self.default_mode_info,
                    self.style,
                    self.session_is_mirrored,
                    multiple_users_exist_in_session,
                )
            };
            let Some(tab) = self.tabs.get_mut(&tab_id) else {
                continue;
            };
            tab.set_global_pane_borders(&borders);
            let strips_changed = tab.set_global_pane_strips(&strips);
            if strips_changed {
                tab.set_force_render();
            }
            if tab.get_display_area() != center_size
                || tab.get_display_area_origin() != (center.x, center.y)
            {
                tab.resize_whole_tab_to(center).context(err_context)?;
                // panes that moved without changing size have empty grid diffs - without a full
                // repaint they paint nothing at their new position and leave a ghost at the old one
                tab.set_force_render();
            } else if strips_changed {
                // the pane geometries did not change, but the panes' boundaries now abut a different
                // set of global panes, so their content offsets need to be recomputed
                tab.reapply_tiled_pane_frames();
            }
            // the tab is now laid out over its center, with the strips it has to leave the global
            // panes - so this is the one place that can have the last word on the chrome's width.
            // Every re-solve above narrows the chrome back to the center and re-widens it only
            // conditionally (a new tab has no strips yet when `apply_layout` resizes it, and a tab
            // whose center and strips are both unchanged is not touched at all), so without this
            // the tab bar and status bar can stay stranded beside a global pane.
            tab.sync_ui_panes_to_global_strips();
        }

        // Separate rendering for regular clients and watchers
        let has_regular_clients = self
            .connected_clients
            .borrow()
            .keys()
            .any(|id| !self.watcher_clients.contains_key(id));
        let has_watchers = !self.watcher_clients.is_empty(); // No change needed
        let global_focused_clients = self.global_panes.focused_clients();

        // Track whether non-watcher output was dirty for conditional watcher rendering
        let non_watcher_output_was_dirty;

        let mut tabs_to_close = vec![];

        // === PHASE 1: Render for regular clients ===
        if has_regular_clients {
            let mut output = Output::new(
                self.sixel_image_store.clone(),
                self.character_cell_size.clone(),
                self.styled_underlines,
                self.osc8_hyperlinks,
                self.kitty_image_store.clone(),
                self.kitty_host_capabilities.clone(),
                self.client_kitty_host_state.clone(),
                self.sixel_host_capabilities.clone(),
            );

            output.collect_ansi_pane_contents =
                self.pane_render_subscribers.values().any(|s| s.ansi);

            // a tab that doesn't render leaves its clients unregistered on the output - see below
            let mut skipped_a_tab = self.tabs.is_empty();
            for (tab_index, tab) in &mut self.tabs {
                if tab.has_selectable_tiled_panes() {
                    // Pass None for normal client rendering
                    tab.render_with_inactive_clients(&mut output, None, &global_focused_clients)
                        .context(err_context)?;
                } else {
                    skipped_a_tab = true;
                    if !tab.is_pending() {
                        tabs_to_close.push(*tab_index);
                    }
                }
            }

            // a tab that cleared the host display wiped the global pane strips too (they live
            // outside its viewport), so they need a full repaint rather than a diff
            if output.any_host_display_cleared() {
                self.global_panes.set_force_render();
            }

            let regular_client_ids: Vec<ClientId> = self
                .connected_clients
                .borrow()
                .keys()
                .filter(|client_id| !self.watcher_clients.contains_key(client_id))
                .copied()
                .collect();
            // Keep global-pane output grouped with the clients viewing each tab.
            let mut clients_by_tab: BTreeMap<Option<usize>, Vec<ClientId>> = BTreeMap::new();
            for client_id in regular_client_ids {
                clients_by_tab
                    .entry(self.active_tab_ids.get(&client_id).copied())
                    .or_default()
                    .push(client_id);
            }
            // notifications paint over the panes, so they're emitted only once every group has
            // rendered - collected here, drawn just below
            let mut notification_targets: Vec<(Viewport, Vec<ClientId>)> = vec![];
            let multiple_users_exist_in_session = self.connected_clients.borrow().len() > 1;
            // ponytail: rendering a pane clears its dirty flag, so with more than one tab in view
            // every group after the first would draw nothing. Repaint each group instead of
            // tracking dirtiness per client - worth splitting only if the repaint cost is
            // measurable.
            let repaint_per_group = clients_by_tab.len() > 1;
            for (tab_id, client_ids) in clients_by_tab {
                let tab_size = tab_id
                    .and_then(|tab_id| self.tab_surface_size(tab_id))
                    .unwrap_or(surface);
                let layout = match self.global_layout_for(tab_size) {
                    Ok(layout) => layout,
                    // as above: this render group goes unpainted, the session lives on
                    Err(error) => {
                        Err::<(), _>(error).context(err_context).non_fatal();
                        continue;
                    },
                };
                self.global_panes.set_positions(&layout);
                if repaint_per_group {
                    self.global_panes.set_force_render();
                }
                let floating_stack = tab_id
                    .and_then(|tab_id| self.tabs.get(&tab_id))
                    .and_then(|tab| tab.floating_panes_stack());
                // the global panes render clipped against these geometries, so the cells behind a
                // floating pane get no chunks and keep whatever that pane last drew on them. When
                // the stack moves, those cells have nothing to diff against - repaint in full or
                // the floating pane leaves a ghost of itself over the global pane
                let floating_stack_changed = match tab_id {
                    Some(tab_id) => {
                        let layers = floating_stack
                            .as_ref()
                            .map(|stack| stack.layers.clone())
                            .unwrap_or_default();
                        let previous = self
                            .last_rendered_floating_stacks
                            .insert(tab_id, layers.clone());
                        previous.as_ref() != Some(&layers)
                    },
                    None => false,
                };
                if floating_stack_changed {
                    self.global_panes.set_force_render();
                }
                // the tab loop above left the output pointing at whichever tab rendered last -
                // point it at this group's tab, or the global panes are clipped against another
                // tab's floating panes and paint over the ones in front of these clients
                let previous_stack = output.set_floating_panes_stack(floating_stack);
                let group_result = (|| -> Result<()> {
                    self.global_panes.render(
                        &mut output,
                        &client_ids,
                        &self.mode_info,
                        &self.default_mode_info,
                        self.style,
                        self.session_is_mirrored,
                        multiple_users_exist_in_session,
                    )?;
                    if let Some(tab_id) = tab_id {
                        // a host display clear wiped these cells along with everything else, and a
                        // floating pane that moved off them left its own contents behind
                        let force = output.any_host_display_cleared() || floating_stack_changed;
                        self.fill_unowned_strip_rows(
                            &mut output,
                            tab_id,
                            &layout,
                            &client_ids,
                            force,
                        )?;
                        if let Some(tab) = self.tabs.get_mut(&tab_id) {
                            tab.render_floating_panes_after_global_panes(
                                &mut output,
                                None,
                                &global_focused_clients,
                            )?;
                        }
                    }
                    Ok(())
                })();
                output.set_floating_panes_stack(previous_stack);
                group_result.context(err_context)?;
                if !self.notifications.is_empty() {
                    notification_targets.push((layout.center, client_ids));
                }
            }

            for (viewport, client_ids) in notification_targets {
                self.render_notifications(&mut output, viewport, &client_ids)
                    .context(err_context)?;
            }

            // GlobalPanes::render clears each pane's should_render, but Output drops chunks for
            // clients no tab registered - a skipped tab would burn those dirty flags, so repaint
            // next frame
            if skipped_a_tab {
                self.global_panes.set_force_render();
            }

            // the cursor is positioned only now, once the global panes' contents are in the frame
            // too - otherwise a frame in which only a global pane rendered would leave the cursor
            // wherever that pane's last character chunk landed
            for tab in self.tabs.values_mut() {
                if tab.has_selectable_tiled_panes() {
                    tab.render_cursor_after_global_panes(&mut output, &global_focused_clients);
                }
            }

            let pane_render_report = output.drain_pane_render_report();

            // Subscriber delivery — gated behind is_empty() for zero overhead
            if !self.pane_render_subscribers.is_empty() {
                self.deliver_to_pane_subscribers_from_report(&pane_render_report);
            }

            let _ = self
                .bus
                .senders
                .send_to_plugin(PluginInstruction::PaneRenderReport(pane_render_report));

            non_watcher_output_was_dirty = output.is_dirty();

            let mut bell_state_changed = false;
            let mut has_bell = false;

            if self.visual_bell {
                let mut panes_to_flash: Vec<PaneId> = vec![];
                let mut tabs_to_flash: Vec<usize> = vec![];

                let active_tab_ids_snapshot: Vec<usize> =
                    self.active_tab_ids.values().copied().collect();

                for tab in self.tabs.values_mut() {
                    let is_active = active_tab_ids_snapshot.contains(&tab.id);
                    let (new_panes, tab_newly_set) =
                        tab.check_and_handle_bell_notifications(is_active);
                    if !new_panes.is_empty() {
                        panes_to_flash.extend(new_panes);
                        bell_state_changed = true;
                    }
                    if tab_newly_set {
                        tabs_to_flash.push(tab.id);
                        bell_state_changed = true;
                    }
                }

                has_bell = !panes_to_flash.is_empty() || !tabs_to_flash.is_empty();
                if !panes_to_flash.is_empty() {
                    let _ = self
                        .bus
                        .senders
                        .send_to_background_jobs(BackgroundJob::FlashPaneBell(panes_to_flash));
                }
                for tab_id in tabs_to_flash {
                    let _ = self
                        .bus
                        .senders
                        .send_to_background_jobs(BackgroundJob::FlashTabBell(tab_id));
                }
            } else {
                // visual_bell disabled: still detect bell for ANSI BEL forwarding only
                for tab in self.tabs.values_mut() {
                    if tab.check_and_consume_bells_without_visual_notification() {
                        has_bell = true;
                    }
                }
            }

            if has_bell {
                output.add_post_vte_instruction_to_multiple_clients(
                    self.active_tab_ids.keys().copied(),
                    "\u{7}", // ANSI BEL
                );
            }

            for (client_id, tab_index) in &self.active_tab_ids {
                if self.watcher_clients.contains_key(client_id) {
                    continue;
                }
                if let Some(tab) = self.tabs.get(tab_index) {
                    let mut visible_pane_ids = tab.kitty_visible_pane_ids();
                    visible_pane_ids.extend(self.global_panes.visible_pane_ids());
                    output.set_kitty_visible_panes(*client_id, visible_pane_ids);
                }
            }

            if non_watcher_output_was_dirty || has_bell {
                let serialized_output = output.serialize().context(err_context)?;
                if !serialized_output.is_empty() {
                    let _ = self
                        .bus
                        .senders
                        .send_to_server(ServerInstruction::Render(Some(serialized_output)))
                        .context(err_context);
                }
            }

            let derived_names_changed = self.update_plugin_tab_names();
            if bell_state_changed || derived_names_changed {
                self.log_and_report_session_state()?;
            }
        } else {
            // No regular clients, output is not dirty
            non_watcher_output_was_dirty = false;

            // No regular clients but subscribers exist — query panes directly
            if !self.pane_render_subscribers.is_empty() {
                self.deliver_to_pane_subscribers_directly();
            }
        }

        // === PHASE 2: Render for watchers ===
        if has_watchers {
            if let Some(followed_client_id) = self.followed_client_id {
                // Create fresh output for watchers
                let mut watcher_output = Output::new(
                    self.sixel_image_store.clone(),
                    self.character_cell_size.clone(),
                    self.styled_underlines,
                    self.osc8_hyperlinks,
                    self.kitty_image_store.clone(),
                    self.kitty_host_capabilities.clone(),
                    self.client_kitty_host_state.clone(),
                    self.sixel_host_capabilities.clone(),
                );

                let focused_tab_index_of_followed_client_id =
                    *self.active_tab_ids.get(&followed_client_id).unwrap_or(&0);
                let followed_content_size =
                    self.followed_frame_size(focused_tab_index_of_followed_client_id);

                // Only force render if:
                // 1. Non-watcher output was dirty, OR
                // 2. Any watcher needs a forced render (first render or after resize), OR
                // 3. No non-watcher clients are connected
                let any_watcher_needs_force_render = self
                    .watcher_clients
                    .values()
                    .any(|state| state.should_force_render());
                let should_force_render = non_watcher_output_was_dirty
                    || any_watcher_needs_force_render
                    || !has_regular_clients;

                if let Some(tab) = self
                    .tabs
                    .get_mut(&focused_tab_index_of_followed_client_id)
                    .as_mut()
                {
                    if should_force_render {
                        tab.set_force_render();
                        self.global_panes.set_force_render();
                    }
                    tab.render_with_inactive_clients(
                        &mut watcher_output,
                        Some(followed_client_id),
                        &global_focused_clients,
                    )
                    .context(err_context)?;
                }

                // phase 1 left the panes at the last render group's tab, which is the followed
                // client's only by luck - the watcher's content and `serialize_with_size` crop both
                // come from the followed tab, so the panes have to sit where they do for it
                let watcher_layout = self.position_global_panes_for_client(followed_client_id);
                self.global_panes
                    .render(
                        &mut watcher_output,
                        &[followed_client_id],
                        &self.mode_info,
                        &self.default_mode_info,
                        self.style,
                        self.session_is_mirrored,
                        self.connected_clients.borrow().len() > 1,
                    )
                    .context(err_context)?;
                if let Some(layout) = watcher_layout.filter(|_| should_force_render) {
                    // this output is the followed client's, cropped per watcher on serialize, and
                    // `should_force_render` already means a full repaint - so no diff to keep
                    self.fill_unowned_strip_rows(
                        &mut watcher_output,
                        focused_tab_index_of_followed_client_id,
                        &layout,
                        &[followed_client_id],
                        true,
                    )
                    .context(err_context)?;
                }
                if let Some(tab) = self
                    .tabs
                    .get_mut(&focused_tab_index_of_followed_client_id)
                    .as_mut()
                {
                    tab.render_floating_panes_after_global_panes(
                        &mut watcher_output,
                        Some(followed_client_id),
                        &global_focused_clients,
                    )
                    .context(err_context)?;
                    tab.render_cursor_after_global_panes(
                        &mut watcher_output,
                        &global_focused_clients,
                    );
                }
                if let Some(tab) = self.tabs.get(&focused_tab_index_of_followed_client_id) {
                    let mut visible_pane_ids = tab.kitty_visible_pane_ids();
                    visible_pane_ids.extend(self.global_panes.visible_pane_ids());
                    watcher_output.set_kitty_visible_panes(followed_client_id, visible_pane_ids);
                }

                // Send the rendered output to all watcher clients
                if watcher_output.is_dirty() {
                    let mut watcher_render_output: HashMap<ClientId, String> = HashMap::new();

                    // For each watcher, clone the output and serialize with size constraints
                    for (watcher_id, watcher_state) in &self.watcher_clients {
                        let mut watcher_specific_output = watcher_output.clone();

                        // Serialize this watcher's output with size constraints (cropping and padding handled inside)
                        let mut serialized_output = watcher_specific_output
                            .serialize_with_size(Some(watcher_state.size()), followed_content_size)
                            .context(err_context)?;

                        // Get the output for the followed client and map it to this watcher
                        if let Some(followed_output) = serialized_output.remove(&followed_client_id)
                        {
                            watcher_render_output.insert(*watcher_id, followed_output);
                        }
                    }

                    // Send to server for delivery to watcher clients
                    if !watcher_render_output.is_empty() {
                        let _ = self
                            .bus
                            .senders
                            .send_to_server(ServerInstruction::Render(Some(watcher_render_output)))
                            .context(err_context);
                    }

                    // Clear force render flag for all watchers after successful render
                    for watcher_state in self.watcher_clients.values_mut() {
                        watcher_state.clear_force_render();
                    }
                }
            }
        }
        for tab_index in tabs_to_close {
            self.close_tab_by_id(tab_index)
                .context(err_context)
                .non_fatal();
        }

        Ok(())
    }

}

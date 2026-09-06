use zellij_runtime::errors::prelude::*;

use super::Screen;
use crate::background_jobs::BackgroundJob;
use crate::output::Output;
use crate::panes::PaneId;
use crate::plugins::{PluginInstruction, PluginRenderAsset};
use crate::{ClientId, ServerInstruction};
use std::collections::HashMap;
use zellij_types::pane_size::Viewport;

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
        self.render_to_clients_traced(false)
    }

    pub(crate) fn render_to_clients_traced(&mut self, trace_attach: bool) -> Result<()> {
        // this method does the actual rendering and is triggered by a debounced BackgroundJob (see
        // the render method for more details)
        let err_context = "failed to render screen";
        let trace_started_at = std::time::Instant::now();
        if trace_attach {
            log::info!("attach trace: render frame setup begin");
        }

        // One instant for the whole frame. Every animation is *sampled* here rather than advanced
        // on the clock tick, so there is no skew between "advance" and "paint" and every frame -
        // whoever scheduled it - moves the animation to where the wall clock says it should be.
        let now = self.frame_now();
        self.reap_settled_notifications(now);
        self.advance_frame_color_fades(now);
        self.advance_agent_attention_blinks();
        self.advance_plugin_loading_animations();

        // Every tab is laid out over the same session surface. Re-derive it here so a render tick
        // cannot restore stale geometry.
        self.remember_surface_size();
        let surface = self.surface_size();
        for tab_id in self.tabs.keys().copied().collect::<Vec<_>>() {
            // With no contributing client, fall back to the last known shared surface.
            let tab_size = self.tab_surface_size(tab_id).unwrap_or(surface);
            let Some(tab) = self.tabs.get_mut(&tab_id) else {
                continue;
            };
            if tab.get_display_area() != tab_size {
                tab.resize_whole_tab_to(tab_size).context(err_context)?;
                // panes that moved without changing size have empty grid diffs - without a full
                // repaint they paint nothing at their new position and leave a ghost at the old one
                tab.set_force_render();
            }
        }
        if trace_attach {
            log::info!(
                "attach trace: render frame setup and tab sizing complete after {:?}",
                trace_started_at.elapsed()
            );
        }

        // Separate rendering for regular clients and watchers
        let has_regular_clients = self
            .connected_clients
            .borrow()
            .iter()
            .any(|id| !self.watcher_clients.contains_key(id));
        let has_watchers = !self.watcher_clients.is_empty(); // No change needed

        // Track whether non-watcher output was dirty for conditional watcher rendering
        let non_watcher_output_was_dirty;

        let mut tabs_to_close = vec![];

        // === PHASE 1: Render for regular clients ===
        if has_regular_clients {
            if trace_attach {
                log::info!("attach trace: render regular-client pane rendering begin");
            }
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

            // Before any pane renders: work out which cells this frame's toasts have stopped
            // covering and ask the panes under them to repaint. A pane only emits chunks for rows
            // its own grid calls dirty, so a toast that slid off a cell would otherwise leave its
            // own border sitting there - a trail that only cleared when the toast was finally
            // removed. Narrowed to the panes actually overlapping the damage, and empty on every
            // frame where the toasts only grew, so an arriving toast costs nothing.
            for (tab_id, damage) in self.prepare_notification_damage(now, surface) {
                if let Some(tab_id) = tab_id {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.force_render_panes_intersecting(&damage);
                    }
                }
            }

            let session_surface = Viewport {
                x: 0,
                y: 0,
                cols: surface.cols,
                rows: surface.rows,
            };

            // The toasts are drawn above every pane, so nothing may put a cursor - real or the
            // fake one standing in for another client - on a cell they cover. Told to the output
            // before anything renders, because the fake cursors are decided while the panes draw.
            output.set_notification_rects(if self.notifications.is_empty() {
                vec![]
            } else {
                self.notification_cell_rects(session_surface, now)
            });

            // Wanted twice below - once for the menu geometry, once when the overlays paint - and
            // on almost every frame there is neither a toast nor a menu, so it is not worth
            // walking the client list to find out.
            let has_overlays = !self.notifications.is_empty() || !self.context_menus.is_empty();
            let regular_client_ids: Vec<ClientId> = if has_overlays {
                self.connected_clients
                    .borrow()
                    .iter()
                    .filter(|client_id| !self.watcher_clients.contains_key(client_id))
                    .copied()
                    .collect()
            } else {
                vec![]
            };

            // ...and the same for the menus, which are drawn above even the toasts. Per client,
            // because a menu belongs to the client that opened it.
            output.set_context_menu_rects(if self.context_menus.is_empty() {
                HashMap::new()
            } else {
                self.context_menu_cell_rects(&regular_client_ids, session_surface)
            });

            // Whether an emptied last tab is to be held open rather than swept, and so whether
            // the confirmation still has to be raised. Read before the loop because the loop
            // borrows `self.tabs` and the answer is about the session, not about a tab.
            let hold_the_last_tab = self.tabs.len() == 1 && self.quit_is_guarded();
            let mut raise_quit_confirmation = false;

            // Only the panes here: the cursor is placed further down, once the toasts have painted
            // too.
            let mut tabs_that_rendered: Vec<usize> = vec![];
            for (tab_index, tab) in &mut self.tabs {
                if tab.has_selectable_tiled_panes() {
                    // Pass None for normal client rendering
                    tab.render_panes(&mut output, None).context(err_context)?;
                    tabs_that_rendered.push(*tab_index);
                    // Something selectable is back, so a later close asks again.
                    self.last_tab_held_open = false;
                } else if !tab.is_pending() {
                    if hold_the_last_tab {
                        // A plugin has something outstanding and asked to be consulted. Draw the
                        // tab anyway - `render_panes` copes with no active tiled pane, and the
                        // overlays are what there is to see - and raise the confirmation once,
                        // on the frame the tab first empties. Cancelling it needs no signal:
                        // the tab stays held until something is opened in it.
                        tab.render_panes(&mut output, None).context(err_context)?;
                        tabs_that_rendered.push(*tab_index);
                        raise_quit_confirmation = !self.last_tab_held_open;
                        self.last_tab_held_open = true;
                    } else {
                        tabs_to_close.push(*tab_index);
                    }
                }
            }
            if raise_quit_confirmation {
                // The lowest client id rather than the first, for the reason `close_tab_by_id`
                // gives when it picks the client its overlays follow: `connected_clients` is a
                // `HashSet`, so "first" is not a stable answer. One client answers for the
                // session, as it does for `Ctrl-q`.
                if let Some(client_id) = self.active_tab_ids.keys().copied().min() {
                    self.confirm_quit(client_id, true, None)
                        .context(err_context)
                        .non_fatal();
                }
            }
            if trace_attach {
                log::info!(
                    "attach trace: render regular-client panes complete after {:?}",
                    trace_started_at.elapsed()
                );
            }

            // notifications paint over the panes, so they're emitted once every tab has rendered
            if has_overlays {
                self.render_notifications(&mut output, session_surface, &regular_client_ids, now)
                    .context(err_context)?;
                // Above the toasts: a menu is something the user opened and is looking at right
                // now, and a toast arriving underneath it must not cover the row they are about
                // to click.
                self.render_context_menus(&mut output, session_surface, &regular_client_ids)
                    .context(err_context)?;
            }

            // The cursor goes after the toasts, not inside each tab's render. `render_cursor` only
            // re-places the cursor when something painted this frame, and a toast repainting on
            // its own is exactly that - a frame whose only chunks belong to a layer the tab cannot
            // see. Deciding before they painted left such a frame with no goto at all, so the
            // cursor stayed wherever the toast's last absolute goto put it: blinking in the
            // bottom-right of the box.
            for tab_index in &tabs_that_rendered {
                if let Some(tab) = self.tabs.get_mut(tab_index) {
                    tab.render_cursor_last(&mut output);
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
                    output.set_kitty_visible_panes(*client_id, tab.kitty_visible_pane_ids());
                }
            }

            if non_watcher_output_was_dirty || has_bell {
                if trace_attach {
                    log::info!("attach trace: render output serialization begin");
                }
                let serialized_output = output.serialize().context(err_context)?;
                if trace_attach {
                    log::info!(
                        "attach trace: render output serialization complete clients={} bytes={} after {:?}",
                        serialized_output.len(),
                        serialized_output.values().map(String::len).sum::<usize>(),
                        trace_started_at.elapsed()
                    );
                }
                if !serialized_output.is_empty() {
                    if trace_attach {
                        log::info!("attach trace: render enqueue to server main loop begin");
                    }
                    let _ = self
                        .bus
                        .senders
                        .send_to_server(ServerInstruction::Render(Some(serialized_output)))
                        .context(err_context);
                    if trace_attach {
                        log::info!(
                            "attach trace: render enqueue to server main loop complete after {:?}",
                            trace_started_at.elapsed()
                        );
                    }
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

                // A watcher is watching a particular client's screen, so it sees that client's
                // menu too - otherwise the watcher would see a pane the followed user is plainly
                // interacting with and no sign of what they are interacting with it through.
                let watcher_surface =
                    self.session_surface_for_tab(focused_tab_index_of_followed_client_id);
                // Before the tab renders, for the reason the regular path gives: the cursor this
                // output places is the followed client's, and it must not be placed on a cell that
                // client's own menu is about to cover.
                if let Some(watcher_surface) = watcher_surface {
                    watcher_output.set_context_menu_rects(
                        self.context_menu_cell_rects(&[followed_client_id], watcher_surface),
                    );
                }
                if let Some(tab) = self
                    .tabs
                    .get_mut(&focused_tab_index_of_followed_client_id)
                    .as_mut()
                {
                    if should_force_render {
                        tab.set_force_render();
                    }
                    tab.render(&mut watcher_output, Some(followed_client_id))
                        .context(err_context)?;
                }
                if let Some(watcher_surface) = watcher_surface {
                    self.render_context_menus(
                        &mut watcher_output,
                        watcher_surface,
                        &[followed_client_id],
                    )
                    .context(err_context)?;
                }
                if let Some(tab) = self.tabs.get(&focused_tab_index_of_followed_client_id) {
                    watcher_output
                        .set_kitty_visible_panes(followed_client_id, tab.kitty_visible_pane_ids());
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

        if trace_attach {
            log::info!(
                "attach trace: render frame complete after {:?}",
                trace_started_at.elapsed()
            );
        }

        Ok(())
    }
}

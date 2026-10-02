use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;

use super::Screen;
use crate::background_jobs::BackgroundJob;
use crate::output::Output;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::{ClientId, ServerInstruction};
use std::collections::HashMap;
use std::time::Duration;
use yinz_types::pane_size::Viewport;

/// Frames a client may have queued behind its in-flight write before it counts as
/// behind. See `Screen::every_client_is_behind_on_renders`.
const MAX_QUEUED_RENDERS: usize = 1;
/// One write can legitimately block for seconds: the client reads the next frame only
/// once its terminal has taken several megabytes of the last. Only a client that has
/// stopped reading should take this long.
const STALLED_CLIENT_WRITE: Duration = Duration::from_secs(30);

impl Screen {
    pub(super) fn watched_tab_id(&self) -> Option<TabId> {
        self.followed_client_id
            .filter(|_| !self.watcher_clients.is_empty())
            .map(|client| {
                self.active_tab_ids
                    .get(&client)
                    .copied()
                    .unwrap_or(TabId::new(0))
            })
    }

    pub fn render(&mut self) -> Result<()> {
        // here we schedule the RenderToClients background job which debounces renders every 10ms
        // rather than actually rendering
        //
        // when this job decides to render, it sends back the ScreenInstruction::RenderToClients
        // message, triggering our render_to_clients method which does the actual rendering

        self.dismiss_stale_command_palettes();
        self.sync_nested_guest_fullscreen_state();
        self.report_pane_tabs()?;

        let _ = self
            .bus
            .senders
            .send_to_background_jobs(BackgroundJob::RenderToClients);
        Ok(())
    }

    /// Whether every client still has a frame queued behind the one it is writing.
    ///
    /// Rendering then would only queue diffs nobody can drain, until the send
    /// queues overflow and the server disconnects them. Skipping the frame loses
    /// nothing: pane dirty state accumulates, and the next frame carries it all.
    ///
    /// Only when *every* client is behind: frames are rendered once for all of them,
    /// so waiting for one slow client would freeze the healthy ones. A slow client
    /// among healthy ones keeps receiving diffs, and the send queue's caps disconnect
    /// it if it cannot keep up. A client whose write has been blocked for
    /// `STALLED_CLIENT_WRITE` has stopped reading, and does not hold frames back either.
    pub fn every_client_is_behind_on_renders(&self) -> bool {
        let Some(os_input) = self.bus.os_input.as_ref() else {
            return false;
        };
        let clients = self.connected_clients.borrow();
        !clients.is_empty()
            && clients.iter().all(|client_id| {
                os_input
                    .client_render_backlog(*client_id)
                    .is_some_and(|backlog| {
                        backlog.queued_renders >= MAX_QUEUED_RENDERS
                            && backlog
                                .in_flight_for
                                .is_none_or(|blocked| blocked < STALLED_CLIENT_WRITE)
                    })
            })
    }

    /// Skip this frame if every client is behind, asking the debouncer to try again;
    /// see `every_client_is_behind_on_renders`. Returns whether it skipped. Polls
    /// every debounce interval until a client drains.
    pub fn defer_render_while_clients_are_behind(&self) -> bool {
        let behind = self.every_client_is_behind_on_renders();
        if behind {
            self.retry_render_later();
        }
        behind
    }

    /// Ask the debouncer for another frame, as `render` does, without the bookkeeping
    /// `render` repeats for each new frame request.
    pub fn retry_render_later(&self) {
        let _ = self
            .bus
            .senders
            .send_to_background_jobs(BackgroundJob::RenderToClients);
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
        self.advance_plugin_loading_animations();

        // Every tab is laid out over the same session surface. Re-derive it here so a render tick
        // cannot restore stale geometry.
        self.remember_surface_size();
        let surface = self.surface_size();
        self.dismiss_stale_command_palettes();
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

        let watched_tab_id = self.watched_tab_id();
        for (tab_id, tab) in &mut self.tabs {
            tab.sample_overlay_ripples(now, self.pane_animations);
            tab.sample_waiting_pulses(
                now,
                self.pane_animations
                    && (self.active_tab_ids.values().any(|id| id == tab_id)
                        || watched_tab_id == Some(*tab_id)),
                self.followed_client_id
                    .filter(|_| watched_tab_id == Some(*tab_id)),
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
                self.synchronized_output_capabilities.clone(),
            );

            self.prepare_command_palette_render(&mut output);

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
            output.set_notification_rects(self.notification_occlusion_rects(session_surface, now));

            // Wanted twice below - once for the menu geometry, once when the overlays paint - and
            // on almost every frame there is neither a toast nor a menu, so it is not worth
            // walking the client list to find out.
            let has_overlays = !self.notifications.is_empty()
                || !self.notification_browsers.is_empty()
                || !self.context_menus.is_empty();
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

            // Only the panes here: the cursor is placed further down, once the toasts have painted
            // too. Whether a tab lives is not decided here - `Screen::settle_tabs` closes an
            // emptied tab after the instruction that emptied it - so an unusable tab reaching
            // this loop is one being kept on purpose: still loading, an arrangement's
            // destination, or the last tab waiting for its replacement. Drawing it is harmless:
            // `render_panes` copes with no active tiled pane, and its overlays are what there
            // is to see.
            let mut tabs_that_rendered: Vec<TabId> = vec![];
            for (tab_index, tab) in &mut self.tabs {
                if tab.should_retain_tab() || !tab.is_pending() {
                    // Pass None for normal client rendering
                    tab.render_panes(&mut output, None).context(err_context)?;
                    tabs_that_rendered.push(*tab_index);
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
                    tab.render_cursor_last(&mut output, None);
                }
            }

            self.render_command_palettes(&mut output, self.command_palettes.keys().copied())
                .context(err_context)?;

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
                let mut tabs_to_flash: Vec<TabId> = vec![];

                let active_tab_ids_snapshot: Vec<TabId> =
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

            let mut has_osc7_update = false;
            for (client_id, tab_index) in &self.active_tab_ids {
                if self.watcher_clients.contains_key(client_id) {
                    continue;
                }
                if let Some(tab) = self.tabs.get(tab_index) {
                    output.set_kitty_visible_panes(*client_id, tab.kitty_visible_pane_ids());

                    let current_osc7 = tab
                        .get_active_pane(*client_id)
                        .and_then(|pane| pane.osc7_payload());
                    let last = self
                        .last_forwarded_osc7
                        .get(client_id)
                        .and_then(|uri| uri.as_deref());
                    if current_osc7 != last {
                        if let Some(uri) = current_osc7 {
                            output.add_post_vte_instruction_to_client(
                                *client_id,
                                &format!("\x1b]7;{}\x1b\\", uri),
                            );
                            has_osc7_update = true;
                        }
                        self.last_forwarded_osc7
                            .insert(*client_id, current_osc7.map(str::to_owned));
                    }
                }
            }

            if non_watcher_output_was_dirty || has_bell || has_osc7_update {
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

        self.report_pane_tabs()?;

        // === PHASE 2: Render for watchers ===
        if has_watchers {
            if let (Some(followed_client_id), Some(focused_tab_index_of_followed_client_id)) =
                (self.followed_client_id, watched_tab_id)
            {
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
                    self.synchronized_output_capabilities.clone(),
                );

                // This frame carries the followed client's id but goes to the watchers' terminals,
                // so what that client's host can do says nothing about it. One stream serves every
                // watcher, so anything short of unanimity keeps hiding the cursor while the frame
                // paints: a hide nobody needed costs a `?25h`, a missing one is a cursor visibly
                // chasing the repaint.
                let every_watcher_presents_frames_atomically = {
                    let capabilities = self.synchronized_output_capabilities.borrow();
                    self.watcher_clients
                        .keys()
                        .all(|watcher_id| capabilities.get(watcher_id).copied().unwrap_or(false))
                };
                watcher_output
                    .set_host_presents_frames_atomically(every_watcher_presents_frames_atomically);

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
                watcher_output.set_input_overlay_clients(self.command_palettes.keys().copied());
                if let Some(tab) = self
                    .tabs
                    .get_mut(&focused_tab_index_of_followed_client_id)
                    .as_mut()
                {
                    if should_force_render {
                        tab.set_force_render();
                        // A forced pane repaint cannot erase overlays outside the panes.
                        watcher_output.add_pre_vte_instruction_to_client(
                            followed_client_id,
                            "\u{1b}[m\u{1b}[2J",
                        );
                        watcher_output.mark_host_display_cleared_for_client(followed_client_id);
                    }
                    if any_watcher_needs_force_render {
                        // A watcher about to be repainted whole has not necessarily seen this
                        // screen before - it may have just attached, resized, or changed which
                        // client it follows, none of which the tab's memo (which is keyed by the
                        // followed client, one entry for every watcher) can tell apart. State the
                        // cursor from scratch for it.
                        tab.forget_watcher_host_cursor_state(followed_client_id);
                    }
                    // A watcher whose last frame ended in a serialization-time `?25l` has a host
                    // cursor the memo does not describe: it is down, and the goto that "placed" it
                    // was clamped to that watcher's edge. Say so before the cursor pass, so the
                    // frame that brings the cursor back into range is itself the frame that says
                    // `?25h` and where - rather than a goto to an already-hidden cursor, with the
                    // correction deferred to a frame nothing will schedule.
                    //
                    // Stated whatever this frame is about to do, rather than only when the cursor
                    // is coming back: where it will land is not knowable here, since `render_panes`
                    // may still groupify a stack and move the active pane out from under it. A
                    // frame that is still cropped pays a `?25h` and a goto that the `?25l` appended
                    // after them undoes - and only ever on a host that presents frames atomically,
                    // which is the only kind that can reach this at all: a watcher whose frame is
                    // painted incrementally has its cursor hidden for the frame, which makes
                    // `must_show` true on its own.
                    let a_watchers_cursor_was_cropped = self
                        .watcher_clients
                        .values()
                        .any(|watcher_state| watcher_state.cursor_was_cropped());
                    if a_watchers_cursor_was_cropped {
                        tab.mark_watcher_host_cursor_cropped(followed_client_id);
                    }
                    tab.render_panes(&mut watcher_output, Some(followed_client_id))
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
                if let Some(tab) = self.tabs.get_mut(&focused_tab_index_of_followed_client_id) {
                    tab.render_cursor_last(&mut watcher_output, Some(followed_client_id));
                    watcher_output
                        .set_kitty_visible_panes(followed_client_id, tab.kitty_visible_pane_ids());
                }
                self.render_command_palettes(
                    &mut watcher_output,
                    std::iter::once(followed_client_id),
                )
                .context(err_context)?;

                // Send the rendered output to all watcher clients
                if watcher_output.is_dirty() {
                    self.render_watcher_output(
                        &watcher_output,
                        followed_client_id,
                        followed_content_size,
                    )?;
                }
            }
        }
        if trace_attach {
            log::info!(
                "attach trace: render frame complete after {:?}",
                trace_started_at.elapsed()
            );
        }

        // Release the clock only after the settled borders have been painted.
        self.sync_animation_clock(now)?;
        Ok(())
    }
    /// Serialize and account only for frames that contain the followed client's stream.
    pub(super) fn render_watcher_output(
        &mut self,
        watcher_output: &Output,
        followed_client_id: ClientId,
        followed_content_size: Option<yinz_types::pane_size::Size>,
    ) -> Result<()> {
        let err_context = "failed to render watcher output";
        let mut watcher_render_output: HashMap<ClientId, String> = HashMap::new();

        // Whether each watcher's copy hid the cursor because it fell outside that
        // watcher's size. Collected rather than applied in place, because the loop
        // borrows `watcher_clients` to read the sizes.
        let mut cursor_was_cropped_for_watcher: Vec<(ClientId, bool)> = vec![];

        // For each watcher, clone the output and serialize with size constraints
        for (watcher_id, watcher_state) in &self.watcher_clients {
            let mut watcher_specific_output = watcher_output.clone();

            // Serialize this watcher's output with size constraints (cropping and padding handled inside)
            let mut serialized_output = watcher_specific_output
                .serialize_with_size(Some(watcher_state.size()), followed_content_size)
                .context(err_context)?;

            // Get the output for the followed client and map it to this watcher
            if let Some(followed_output) = serialized_output.remove(&followed_client_id) {
                cursor_was_cropped_for_watcher.push((
                    *watcher_id,
                    watcher_specific_output
                        .cursor_is_cropped_for(followed_client_id, watcher_state.size()),
                ));
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
        // What each watcher's delivered frame did to its cursor, for the next frame's cursor pass
        // to correct the tab's memo with when the cursor comes back into range.
        for (watcher_id, cursor_was_cropped) in cursor_was_cropped_for_watcher {
            if let Some(watcher_state) = self.watcher_clients.get_mut(&watcher_id) {
                watcher_state.set_cursor_was_cropped(cursor_was_cropped);
            }
        }
        Ok(())
    }
}

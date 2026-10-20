use std::collections::BTreeMap;
use std::time::Instant;
use yinz_types::internal_ids::NotificationId;
use yinz_types::internal_ids::TabId;

use yinz_runtime::errors::prelude::*;

use super::{NotificationClick, NotificationLayout, NotificationRecord, Screen};
use crate::output::Output;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::ui::damage::{overlay_damage, CellRect};
use crate::ui::notification::{
    notification_chunks, notification_height, notification_lines, notification_origins,
    notification_reveal, notification_reveal_bounds, notification_width, settled_row_delta,
    DISPLACE_DURATION, DISPLACE_EASING, DISPLACE_SPAN, EXIT_DURATION, EXIT_EASING,
};
use crate::ClientId;
use yinz_types::animation::scaled_duration;
use yinz_types::input::actions::Action;
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::{Size, Viewport};
use yinz_types::position::Position;

impl Screen {
    fn notification_is_actionable(&self, record: &NotificationRecord) -> bool {
        record.on_click.is_some()
            || record
                .pane_id
                .is_some_and(|pane| self.tab_containing_pane(pane).is_some())
    }

    pub(super) fn pending_notification(&self) -> Option<&NotificationRecord> {
        self.notification_history.iter().rev().find(|record| {
            record.pending
                && (self.notification_is_actionable(record)
                    || self.notifications.iter().any(|notification| {
                        notification.id == record.info.id && !notification.is_dismissing()
                    }))
        })
    }

    pub(super) fn consume_notification(&mut self, id: NotificationId, now: Instant) {
        self.with_notification_stack_change(now, |screen| {
            if let Some(record) = screen
                .notification_history
                .iter_mut()
                .find(|record| record.info.id == id)
            {
                record.pending = false;
            }
        });
    }

    fn notification_activation_hint(
        &self,
        client_id: ClientId,
        target: &NotificationRecord,
    ) -> Option<String> {
        let mode_info = self
            .mode_info
            .get(&client_id)
            .unwrap_or(&self.default_mode_info);
        let (_, bindings) = mode_info
            .keybinds
            .iter()
            .find(|(mode, _)| *mode == mode_info.mode)?;
        let key = bindings
            .iter()
            .filter(|(_, actions)| actions.contains(&Action::ActivateLastNotification))
            .map(|(key, _)| key)
            .min()?;
        let shortcut = key
            .key_modifiers
            .iter()
            .map(ToString::to_string)
            .chain(std::iter::once(key.bare_key.to_string()))
            .collect::<Vec<_>>()
            .join("+");
        let action = if self.notification_is_actionable(target) {
            "activate"
        } else {
            "dismiss"
        };
        Some(format!("{shortcut} to {action}"))
    }

    fn notification_stack_width(
        &self,
        cols: usize,
        target: Option<NotificationId>,
        footer: Option<&str>,
    ) -> Option<usize> {
        self.notifications
            .iter()
            .flat_map(|notification| {
                let footer = footer.filter(|_| target == Some(notification.id));
                std::iter::once(notification.text.as_str()).chain(footer)
            })
            .filter_map(|text| notification_width(text, cols))
            .max()
    }

    /// Lay out live notifications once for both rendering and mouse hit-testing.
    ///
    /// Called once every frame while any notification is live, rather than diffed like
    /// `fill_unowned_strip_rows` - a pane that repaints would overwrite the toast's cells, and
    /// re-emitting after every pane is both correct and cheap. The floating pane stack is cleared
    /// first so `Output` doesn't clip these chunks behind a floating pane sitting in that corner.
    ///
    /// Every toast in the stack shares one box width - the widest any of them wants - so the text
    /// can only be wrapped once that width is settled. One consequence: a new wide toast re-wraps,
    /// and can change the height of, every other live toast on the frame it arrives. Nothing caches
    /// the result, which is what keeps that honest; the wrapping is noise beside the chunk building
    /// already happening here, so it does not want a cache that would go stale on a resize.
    ///
    /// Both animations are sampled at the caller's `now` rather than read from an accumulator, so
    /// every client laid out in one frame sees the same instant. The settled origins are computed
    /// fresh for whichever `viewport` this call was given - so a toast's resting place is exactly
    /// right in every client's view, however their viewports differ - and each toast's
    /// `y_offset` is added on top as a transient displacement that is on its way to zero.
    pub(super) fn notification_layout(
        &self,
        viewport: Viewport,
        now: Instant,
        client_id: ClientId,
    ) -> Vec<NotificationLayout<'_>> {
        let target = self.pending_notification();
        let hint = target.and_then(|target| self.notification_activation_hint(client_id, target));
        let target = target.map(|record| record.info.id);
        let Some(width) = self.notification_stack_width(viewport.cols, target, hint.as_deref())
        else {
            return vec![];
        };
        let wrapped: Vec<(Vec<String>, Option<String>)> = self
            .notifications
            .iter()
            .map(|notification| {
                (
                    notification_lines(&notification.text, width),
                    hint.clone().filter(|_| Some(notification.id) == target),
                )
            })
            .collect();
        let heights: Vec<usize> = wrapped
            .iter()
            .map(|(lines, footer)| notification_height(lines.len() + usize::from(footer.is_some())))
            .collect();
        let origins = notification_origins(viewport, self.notification_position, &heights, width);

        self.notifications
            .iter()
            .zip(wrapped)
            .zip(origins)
            .map(|((notification, (lines, footer)), (x, y))| {
                // the displacement is signed and can point off the top of the viewport for a row
                // or two mid-glide; clamp rather than wrap, or a `usize` underflow would paint the
                // toast at the bottom of the screen
                let y = (y as f32 + notification.y_offset.value(now))
                    .round()
                    .max(0.0) as usize;
                let reveal = notification.reveal.value(now);
                let (revealed_x, visible_cols) =
                    notification_reveal_bounds(x, width, self.notification_position, reveal);
                NotificationLayout {
                    notification,
                    lines,
                    footer,
                    x,
                    y,
                    width,
                    reveal,
                    revealed_x,
                    visible_cols,
                }
            })
            .collect()
    }

    /// The heights of the current stack, at the width the whole session shares.
    ///
    /// Used only to work out how far a stack change moved each toast (`settled_row_delta`), which
    /// is why one canonical width is enough: the displacement it feeds is a transient, and every
    /// client's *settled* position is computed against that client's own viewport regardless. A
    /// client with a narrower viewport or a different binding can therefore differ mid-glide,
    /// and is never out at rest. The widest active hint supplies the canonical footer geometry.
    fn canonical_stack_heights(&self) -> Vec<(NotificationId, usize)> {
        let cols = self.surface_size().cols;
        let target = self.pending_notification();
        let hint = self
            .connected_clients
            .borrow()
            .iter()
            .filter_map(|client| {
                target.and_then(|target| self.notification_activation_hint(*client, target))
            })
            .max_by_key(|hint| notification_width(hint, cols));
        let target = target.map(|record| record.info.id);
        let Some(width) = self.notification_stack_width(cols, target, hint.as_deref()) else {
            return vec![];
        };
        self.notifications
            .iter()
            .map(|notification| {
                (
                    notification.id,
                    notification_height(
                        notification_lines(&notification.text, width).len()
                            + usize::from(hint.is_some() && Some(notification.id) == target),
                    ),
                )
            })
            .collect()
    }

    /// Re-aim every survivor's displacement so a stack change is a glide rather than a jump.
    ///
    /// Each toast's painted row is `settled + y_offset`. When the stack changes, `settled` moves by
    /// `settled_row_delta`; adding that same delta to `y_offset` leaves the painted row exactly
    /// where it was, and re-aiming at zero from there walks it to its new home. That is the whole
    /// fix for a second toast arriving mid-glide: nothing is recomputed from a stored "before"
    /// layout, so nothing can snap to a position it was never at.
    fn retarget_notification_displacement(
        &mut self,
        before: &[(NotificationId, usize)],
        after: &[(NotificationId, usize)],
        now: Instant,
    ) {
        let position = self.notification_position;
        for notification in self.notifications.iter_mut() {
            let Some(delta) = settled_row_delta(position, before, after, notification.id) else {
                continue;
            };
            if delta == 0 {
                continue;
            }
            // painted row = settled row + offset. The settled row just moved by `delta`, so the
            // offset has to move by *minus* delta for the painted row to come out unchanged - that
            // is the whole trick, and getting the sign wrong sends the toast gliding in from the
            // wrong side before settling in the right place.
            let from = notification.y_offset.value(now) - delta as f32;
            notification.y_offset.redirect(
                now,
                from,
                0.0,
                // speed, not duration: a one-row correction should not take as long as a whole
                // toast's worth of displacement
                scaled_duration(from, DISPLACE_SPAN, DISPLACE_DURATION),
                DISPLACE_EASING,
            );
        }
    }

    /// Note that the stack is about to change, run `change`, then glide the survivors into place.
    ///
    /// The heights have to be measured on both sides of the change - a wide toast arriving
    /// re-wraps every other toast and can change their heights too - so this brackets the mutation
    /// rather than leaving each caller to remember.
    pub(super) fn with_notification_stack_change(
        &mut self,
        now: Instant,
        change: impl FnOnce(&mut Self),
    ) {
        let before = self.canonical_stack_heights();
        change(self);
        let after = self.canonical_stack_heights();
        self.retarget_notification_displacement(&before, &after, now);
    }

    /// The cells the toasts paint into `viewport` at `now`.
    ///
    /// One mapping shared by everything that needs to know where the toasts are this frame: the
    /// damage diff below, and the cursor occlusion in `Output`. Deriving both from here is what
    /// stops the repaint geometry and the occlusion geometry drifting apart.
    pub(super) fn notification_cell_rects(
        &self,
        viewport: Viewport,
        now: Instant,
        client_id: ClientId,
    ) -> Vec<CellRect> {
        self.notification_layout(viewport, now, client_id)
            .iter()
            .map(|layout| CellRect {
                x: layout.revealed_x,
                y: layout.y,
                cols: layout.visible_cols,
                rows: layout.height(),
            })
            .collect()
    }

    /// The cells this frame's toasts will paint for each regular client, and the cells last
    /// frame's toasts painted that these do not.
    ///
    /// Run *before* any pane renders, which is the whole point: a pane only emits chunks for rows
    /// its own grid thinks are dirty, so a toast that has moved off a cell leaves its own glyph
    /// there until something repaints it. Repairing that a frame late would show a one-frame trail
    /// at every step of a slide-out; repairing it here means the pane underneath is asked to
    /// repaint in the same frame the cell is uncovered.
    ///
    /// The layout is recomputed by `render_notifications` further down the same frame. It is
    /// sampled at the same `now`, so the two agree exactly - and this is no more work than the
    /// mouse hit-test already costs.
    pub(super) fn prepare_notification_damage(
        &mut self,
        now: Instant,
        surface: Size,
    ) -> Vec<(Option<TabId>, Vec<CellRect>)> {
        // Almost every frame in a session's life has no toast anywhere near it. Bail before
        // touching the client list or solving any geometry, but only once the *last* frame had no
        // toasts either - the frame after the final one still has damage to repair.
        if self.notifications.is_empty()
            && self.notification_browsers.is_empty()
            && self.last_notification_fill.is_empty()
        {
            return vec![];
        }
        let regular_client_ids: Vec<ClientId> = self
            .connected_clients
            .borrow()
            .iter()
            .filter(|client_id| !self.watcher_clients.contains_key(client_id))
            .copied()
            .collect();
        if regular_client_ids.is_empty() {
            return vec![];
        }

        let mut clients_by_tab: BTreeMap<Option<TabId>, Vec<ClientId>> = BTreeMap::new();
        for client_id in regular_client_ids {
            clients_by_tab
                .entry(self.active_tab_ids.get(&client_id).copied())
                .or_default()
                .push(client_id);
        }

        let mut damage_by_tab = vec![];
        for (tab_id, client_ids) in clients_by_tab {
            let tab_size = tab_id
                .and_then(|tab_id| self.tab_surface_size(tab_id))
                .unwrap_or(surface);
            let viewport = Viewport {
                x: 0,
                y: 0,
                cols: tab_size.cols,
                rows: tab_size.rows,
            };
            let mut damage: Vec<CellRect> = vec![];
            for client_id in &client_ids {
                // a browsing client is shown its browser instead of the toasts, so that is what
                // its cells are diffed against
                let painted = if self.notification_browser_is_visible(*client_id) {
                    self.notification_browser_rects(*client_id, viewport)
                } else {
                    self.notification_cell_rects(viewport, now, *client_id)
                };
                // remove rather than insert an empty vec when nothing is painted, so the map
                // empties out again once the last toast is gone and the early-out above can fire
                let previous = if painted.is_empty() {
                    self.last_notification_fill.remove(client_id)
                } else {
                    self.last_notification_fill
                        .insert(*client_id, painted.clone())
                }
                .unwrap_or_default();
                let exposed = overlay_damage(&previous, &painted);
                // No pane supplies replacement chunks for empty floating-only tab areas.
                self.invalidate_command_palette_backing(*client_id, &exposed);
                for rect in exposed {
                    if !damage.contains(&rect) {
                        damage.push(rect);
                    }
                }
            }
            if damage.is_empty() {
                continue;
            }
            // the toast also covers strip rows no pane owns, and that fill is diffed against a
            // cache of its own - drop the entry so this frame repaints it rather than deciding
            // nothing changed
            for client_id in &client_ids {
                self.unowned_strip_fill.remove(client_id);
            }
            damage_by_tab.push((tab_id, damage));
        }
        damage_by_tab
    }

    /// The overlay cells visible to this client, shared by real and fake cursor occlusion.
    pub(super) fn notification_occlusion_rects(
        &self,
        viewport: Viewport,
        now: Instant,
        client_id: ClientId,
    ) -> Vec<CellRect> {
        if self.notification_browser_is_visible(client_id) {
            self.notification_browser_rects(client_id, viewport)
        } else {
            self.notification_cell_rects(viewport, now, client_id)
        }
    }

    pub(super) fn render_notifications(
        &self,
        output: &mut Output,
        viewport: Viewport,
        client_ids: &[ClientId],
        now: Instant,
    ) -> Result<()> {
        if client_ids.is_empty()
            || (self.notifications.is_empty() && self.notification_browsers.is_empty())
        {
            return Ok(());
        }
        let previous_stack = output.set_floating_panes_stack(None);
        let result = (|| -> Result<()> {
            for client_id in client_ids.iter().copied() {
                if self.notification_browser_is_visible(client_id) {
                    self.render_notification_browser(output, viewport, client_id)?;
                    continue;
                }
                for layout in self.notification_layout(viewport, now, client_id) {
                    let chunks = notification_chunks(
                        &layout.lines,
                        layout.footer.as_deref(),
                        layout.x,
                        layout.y,
                        layout.width,
                        &self.style,
                        layout.reveal,
                    );
                    let (chunks, _) = notification_reveal(
                        &chunks,
                        layout.x,
                        layout.width,
                        self.notification_position,
                        layout.reveal,
                    );
                    output.add_character_chunks_to_multiple_clients(
                        chunks,
                        std::iter::once(client_id),
                        None,
                    )?;
                }
            }
            Ok(())
        })();
        output.set_floating_panes_stack(previous_stack);
        result.context("failed to render notifications")
    }

    /// Which toast is under `position`, if it is one a click means anything to.
    ///
    /// A toast is only a click target if it has somewhere to send the click: a pane to focus, a
    /// plugin to tell, or both. One with neither - a plain `notify`, or `yinz action notify` -
    /// is decoration, and a click over it must fall through to the pane underneath rather than
    /// being swallowed by a thing that would do nothing with it.
    fn notification_at(
        &self,
        viewport: Viewport,
        position: Position,
        now: Instant,
        client_id: ClientId,
    ) -> Option<(NotificationId, Option<PaneId>, Option<NotificationClick>)> {
        let line = usize::try_from(position.line()).ok()?;
        self.notification_layout(viewport, now, client_id)
            .into_iter()
            .rev()
            .find(|layout| {
                layout.visible_cols > 0
                    && (layout.revealed_x..layout.revealed_x + layout.visible_cols)
                        .contains(&position.column())
                    && (layout.y..layout.y + layout.height()).contains(&line)
            })
            .map(|layout| layout.notification)
            .filter(|notification| {
                notification.pane_id.is_some() || notification.on_click.is_some()
            })
            .map(|notification| {
                (
                    notification.id,
                    notification.pane_id,
                    notification.on_click.clone(),
                )
            })
    }

    /// Start a toast sliding back out. Informational entries stop being pending immediately;
    /// actionable entries remain pending. The toast stays until its reveal reaches zero.
    ///
    /// The exit is re-aimed from wherever the toast currently is and given a duration proportional
    /// to how much of it is showing, so one dismissed a third of the way in leaves in about a third
    /// of the time rather than snapping to full width and sliding back out from there.
    pub(super) fn dismiss_notification(&mut self, id: NotificationId, now: Instant) -> Result<()> {
        if self.notification_history.iter().any(|record| {
            record.info.id == id && record.pending && !self.notification_is_actionable(record)
        }) {
            self.consume_notification(id, now);
        }
        if let Some(notification) = self.notifications.iter_mut().find(|n| n.id == id) {
            if !notification.is_dismissing() {
                let revealed = notification.reveal.value(now);
                notification.reveal.redirect(
                    now,
                    revealed,
                    0.0,
                    scaled_duration(revealed, 1.0, EXIT_DURATION),
                    EXIT_EASING,
                );
                self.sync_animation_clock(now)?;
            }
        }
        Ok(())
    }

    pub(super) fn handle_pane_notification_click(
        &mut self,
        event: &MouseEvent,
        viewport: Viewport,
        client_id: ClientId,
        now: Instant,
    ) -> bool {
        if event.event_type != MouseEventType::Press
            || !event.left
            || event.right
            || event.middle
            || event.shift
            || event.alt
            || event.ctrl
        {
            return false;
        }
        let Some((notification_id, pane_id, on_click)) =
            self.notification_at(viewport, event.position, now, client_id)
        else {
            return false;
        };
        self.activate_notification(notification_id, pane_id, on_click, client_id, now);
        true
    }

    /// Consume the pending entry, dismiss its toast, bring back the pane it names, and tell the
    /// plugin that raised it. History remains available for explicit repeat activation.
    ///
    /// Shared by a click on a toast and by the notification browser, which is what makes Enter on
    /// an entry "exactly a click". A notification with neither a pane nor a plugin is only
    /// dismissed.
    pub(super) fn activate_notification(
        &mut self,
        notification_id: NotificationId,
        pane_id: Option<PaneId>,
        on_click: Option<NotificationClick>,
        client_id: ClientId,
        now: Instant,
    ) {
        self.consume_notification(notification_id, now);
        self.dismiss_notification(notification_id, now).non_fatal();
        if let Some(pane_id) =
            pane_id.filter(|pane_id| self.tab_containing_pane(*pane_id).is_some())
        {
            if self.get_active_pane_id(&client_id) != Some(pane_id) {
                self.focus_pane_with_id(pane_id, true, false, client_id)
                    .non_fatal();
            }
            self.clear_bell_for_pane_id(pane_id, client_id);
            self.sync_scroll_mode_on_focus(client_id).non_fatal();
            self.log_and_report_session_state().non_fatal();
        }
        // After the focus, so a plugin that acts on the click is acting on a pane the server has
        // already brought back - and unconditionally, because the focus above is skipped when the
        // toast's pane is already this client's active one, which is not a reason for the plugin
        // that raised it to hear nothing.
        if let Some(on_click) = on_click {
            // Both clients, because which of them the message has to be addressed to depends on
            // whether the plugin is shared, and only the plugin thread knows that. See
            // `plugins::client_for_notification_click`.
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::NotificationClicked {
                    plugin_id: on_click.plugin_id,
                    raiser: on_click.client_id,
                    clicker: client_id,
                    message: on_click.message,
                    payload: on_click.payload,
                })
                .context("failed to tell a plugin its notification was clicked")
                .non_fatal();
        }
        self.notify_plugins_of_input(client_id);
        self.render().non_fatal();
    }

    /// Drop any toast that has finished sliding out, gliding the survivors into the gap it leaves.
    ///
    /// Called from `render_to_clients` rather than from the animation tick, so a clock that never
    /// fires - a stalled screen thread, a dead task - cannot strand a dismissed toast in the stack.
    /// Whatever renders next reaps it.
    pub(super) fn reap_settled_notifications(&mut self, now: Instant) {
        if !self.notifications.iter().any(|n| n.is_gone(now)) {
            return;
        }
        self.with_notification_stack_change(now, |screen| {
            screen.notifications.retain(|n| !n.is_gone(now));
        });
    }

    /// Whether any toast still wants a frame.
    ///
    /// Derived from the toasts themselves rather than from a registration list, so it cannot get
    /// stuck reporting `true` and leave the clock running over an idle session.
    pub(super) fn notifications_are_animating(&self, now: Instant) -> bool {
        self.notifications.iter().any(|n| !n.is_settled(now))
    }
}

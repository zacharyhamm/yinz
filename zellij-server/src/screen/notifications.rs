use zellij_utils::errors::prelude::*;

use super::{
    NotificationLayout, Screen, NOTIFICATION_DISPLACEMENT_STEP,
    NOTIFICATION_HORIZONTAL_PROGRESS_STEP,
};
use crate::background_jobs::BackgroundJob;
use crate::output::Output;
use crate::panes::PaneId;
use crate::ui::notification::{
    lerp_origin, notification_chunks, notification_height, notification_lines,
    notification_origins, notification_reveal, notification_reveal_bounds, notification_width,
};
use crate::ClientId;
use zellij_utils::input::mouse::{MouseEvent, MouseEventType};
use zellij_utils::pane_size::Viewport;
use zellij_utils::position::Position;

impl Screen {
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
    /// Entering/exiting toasts slide in from - and back out to - the anchored corner
    /// (`notification_reveal`, driven by each toast's own `horizontal_progress`), and while
    /// `notification_displacement_progress` is still settling, survivors are lerped from where
    /// they sat in `notification_stack_before_change` to where they sit now. Both the "before" and
    /// "after" stacks are laid out fresh for whichever `viewport` this call was given - this can
    /// run once per distinct client viewport in a frame - rather than trusting a stored row number
    /// that might belong to a different-sized viewport.
    fn notification_layout(&self, viewport: Viewport) -> Vec<NotificationLayout<'_>> {
        let Some(width) = self
            .notifications
            .iter()
            .filter_map(|notification| notification_width(&notification.text, viewport.cols))
            .max()
        else {
            return vec![];
        };
        let wrapped: Vec<Vec<String>> = self
            .notifications
            .iter()
            .map(|notification| notification_lines(&notification.text, width))
            .collect();
        let heights: Vec<usize> = wrapped
            .iter()
            .map(|lines| notification_height(lines.len()))
            .collect();
        let origins = notification_origins(viewport, self.notification_position, &heights, width);
        let before_origins = if self.notification_displacement_progress < 1.0 {
            let before_heights: Vec<usize> = self
                .notification_stack_before_change
                .iter()
                .map(|notification| {
                    notification_height(notification_lines(&notification.text, width).len())
                })
                .collect();
            notification_origins(viewport, self.notification_position, &before_heights, width)
        } else {
            vec![]
        };

        self.notifications
            .iter()
            .zip(wrapped)
            .zip(origins)
            .map(|((notification, lines), (x, y))| {
                let (x, y) = if self.notification_displacement_progress < 1.0 {
                    self.notification_stack_before_change
                        .iter()
                        .position(|before| before.id == notification.id)
                        .and_then(|index| before_origins.get(index))
                        .map(|&before_origin| {
                            lerp_origin(
                                before_origin,
                                (x, y),
                                self.notification_displacement_progress,
                            )
                        })
                        .unwrap_or((x, y))
                } else {
                    (x, y)
                };
                let (revealed_x, visible_cols) = notification_reveal_bounds(
                    x,
                    width,
                    self.notification_position,
                    notification.horizontal_progress,
                );
                NotificationLayout {
                    notification,
                    lines,
                    x,
                    y,
                    width,
                    revealed_x,
                    visible_cols,
                }
            })
            .collect()
    }

    pub(super) fn render_notifications(
        &self,
        output: &mut Output,
        viewport: Viewport,
        client_ids: &[ClientId],
    ) -> Result<()> {
        if self.notifications.is_empty() || client_ids.is_empty() {
            return Ok(());
        }
        let layouts = self.notification_layout(viewport);

        let previous_stack = output.set_floating_panes_stack(None);
        let result = (|| -> Result<()> {
            for layout in layouts {
                let chunks = notification_chunks(
                    &layout.lines,
                    layout.x,
                    layout.y,
                    layout.width,
                    &self.style,
                );
                let (chunks, _) = notification_reveal(
                    &chunks,
                    layout.x,
                    layout.width,
                    self.notification_position,
                    layout.notification.horizontal_progress,
                );
                output.add_character_chunks_to_multiple_clients(
                    chunks,
                    client_ids.iter().copied(),
                    None,
                )?;
            }
            Ok(())
        })();
        output.set_floating_panes_stack(previous_stack);
        result.context("failed to render notifications")
    }

    fn pane_notification_at(
        &self,
        viewport: Viewport,
        position: Position,
    ) -> Option<(u64, PaneId)> {
        let line = usize::try_from(position.line()).ok()?;
        self.notification_layout(viewport)
            .into_iter()
            .rev()
            .find(|layout| {
                layout.visible_cols > 0
                    && (layout.revealed_x..layout.revealed_x + layout.visible_cols)
                        .contains(&position.column())
                    && (layout.y..layout.y + notification_height(layout.lines.len()))
                        .contains(&line)
            })
            .and_then(|layout| {
                layout
                    .notification
                    .pane_id
                    .map(|pane_id| (layout.notification.id, pane_id))
            })
    }

    pub(super) fn dismiss_notification(&mut self, id: u64) -> Result<()> {
        if let Some(notification) = self.notifications.iter_mut().find(|n| n.id == id) {
            if !notification.dismissing {
                notification.dismissing = true;
                self.bus
                    .senders
                    .send_to_background_jobs(BackgroundJob::AnimateNotifications)?;
            }
        }
        Ok(())
    }

    pub(super) fn handle_pane_notification_click(
        &mut self,
        event: &MouseEvent,
        viewport: Viewport,
        client_id: ClientId,
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
        let Some((notification_id, pane_id)) = self.pane_notification_at(viewport, event.position)
        else {
            return false;
        };

        self.dismiss_notification(notification_id).non_fatal();
        let pane_exists = self.global_panes.pane(pane_id).is_some()
            || self
                .tabs
                .values()
                .any(|tab| tab.has_pane_with_pid(&pane_id));
        if pane_exists {
            if self.get_active_pane_id(&client_id) != Some(pane_id) {
                self.focus_pane_with_id(pane_id, true, false, client_id)
                    .non_fatal();
            }
            self.clear_bell_for_pane_id(pane_id, client_id);
            self.reconcile_single_pane_focus(client_id);
            self.sync_scroll_mode_on_focus(client_id).non_fatal();
            self.log_and_report_session_state().non_fatal();
        }
        self.notify_plugins_of_input(client_id);
        self.render(None).non_fatal();
        true
    }

    /// One tick of the toast slide/displacement animation: advances every notification's
    /// `horizontal_progress` and the shared `notification_displacement_progress`, finally removing
    /// any toast that has finished sliding out. Returns whether anything is still mid-animation, so
    /// the caller knows whether to keep the animation clock ticking.
    pub(super) fn advance_notification_animations(&mut self) -> bool {
        for notification in self.notifications.iter_mut() {
            let step = if notification.dismissing {
                -NOTIFICATION_HORIZONTAL_PROGRESS_STEP
            } else {
                NOTIFICATION_HORIZONTAL_PROGRESS_STEP
            };
            notification.horizontal_progress =
                (notification.horizontal_progress + step).clamp(0.0, 1.0);
        }

        let was_showing = self.notifications.len();
        if self
            .notifications
            .iter()
            .any(|n| n.dismissing && n.horizontal_progress <= 0.0)
        {
            // the stack is about to shrink - remember the fuller layout so the survivors glide to
            // their new position instead of jumping into the gap
            self.notification_stack_before_change = self.notifications.clone();
            self.notification_displacement_progress = 0.0;
            self.notifications
                .retain(|n| !(n.dismissing && n.horizontal_progress <= 0.0));
        }
        if self.notifications.len() != was_showing {
            // nothing under the toast is dirty, so without forcing a repaint its cells would keep
            // the glyphs it just gave up
            for tab in self.tabs.values_mut() {
                tab.set_force_render();
            }
            self.global_panes.set_force_render();
        }

        if self.notification_displacement_progress < 1.0 {
            self.notification_displacement_progress =
                (self.notification_displacement_progress + NOTIFICATION_DISPLACEMENT_STEP).min(1.0);
        }

        self.notifications
            .iter()
            .any(|n| n.dismissing || n.horizontal_progress < 1.0)
            || self.notification_displacement_progress < 1.0
    }
}

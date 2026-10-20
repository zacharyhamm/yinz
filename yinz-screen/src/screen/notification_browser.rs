//! The notification browser: `Notifications` mode's in-place list of this session's toasts.
//!
//! While a client is in the mode, the corner its toasts stack in shows every notification the
//! session has received, newest first, in place of the live toasts. The list
//! is keyboard-navigable, and activating an entry does exactly what clicking its toast does - see
//! `Screen::activate_notification` - then leaves the mode.
//!
//! Per client, because the input mode is: two clients can browse different selections, and a
//! client that is not browsing keeps seeing its toasts.

use std::time::Instant;

use yinz_runtime::errors::prelude::*;
use yinz_types::geometry::Direction;
use yinz_types::internal_ids::NotificationId;
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::Viewport;

use super::{NotificationRecord, Screen};
use crate::output::Output;
use crate::ui::damage::CellRect;
use crate::ui::notification::{
    notification_box_chunks, notification_browser_layout, notification_height, BrowserLayout,
};
use crate::{ClientId, ServerInstruction};

/// What an empty browser says, so entering the mode with nothing to list still shows the mode did
/// something.
const EMPTY_BROWSER_TEXT: &str = "No notifications";

#[derive(Debug, Clone, Default)]
pub(super) struct NotificationBrowser {
    /// The selected entry, by id rather than by index so a toast arriving mid-browse - which lands
    /// at the top of the list - does not move the selection onto a different notification. Pinned
    /// to the newest entry on open; `None` only while there is nothing to select.
    selected: Option<NotificationId>,
    /// Where the visible window started last frame; see `notification_browser_layout`.
    first: usize,
    /// Whether the selected entry shows its full text rather than its first rows.
    expanded: bool,
}

/// One client's browser resolved against the current history and a viewport.
pub(super) struct BrowserFrame {
    pub(super) layout: BrowserLayout,
    /// The id behind each laid-out index. Empty when the list is, and the one box on screen is the
    /// placeholder.
    pub(super) ids: Vec<NotificationId>,
    pub(super) selected: usize,
}

impl Screen {
    pub(super) fn notification_browser_is_visible(&self, client_id: ClientId) -> bool {
        self.notification_browsers.contains_key(&client_id)
            && !self.command_palette_is_open(client_id)
    }

    /// The notifications the browser lists, newest first.
    fn browser_entries(&self) -> Vec<&NotificationRecord> {
        self.notification_history.iter().rev().collect()
    }

    fn browser_selected_index(
        browser: &NotificationBrowser,
        entries: &[&NotificationRecord],
    ) -> usize {
        browser
            .selected
            .and_then(|id| entries.iter().position(|record| record.info.id == id))
            .unwrap_or(0)
    }

    pub(super) fn open_notification_browser(&mut self, client_id: ClientId) {
        let selected = self.browser_entries().first().map(|record| record.info.id);
        self.notification_browsers.insert(
            client_id,
            NotificationBrowser {
                selected,
                ..NotificationBrowser::default()
            },
        );
    }

    pub(super) fn close_notification_browser(&mut self, client_id: ClientId) {
        self.notification_browsers.remove(&client_id);
    }

    /// A notification arrived: keep every open browser's window on the entries it was showing. A
    /// browser that had nothing to select selects the arrival, so the next one cannot take it.
    pub(super) fn notification_browsers_saw_arrival(&mut self) {
        let newest = self.browser_entries().first().map(|record| record.info.id);
        for browser in self.notification_browsers.values_mut() {
            if browser.selected.is_some() {
                browser.first += 1;
            } else {
                browser.selected = newest;
            }
        }
    }

    pub(super) fn notification_browser_frame(
        &self,
        client_id: ClientId,
        viewport: Viewport,
    ) -> Option<BrowserFrame> {
        let browser = self.notification_browsers.get(&client_id)?;
        let entries = self.browser_entries();
        let selected = Self::browser_selected_index(browser, &entries);
        let texts: Vec<String> = if entries.is_empty() {
            vec![EMPTY_BROWSER_TEXT.to_owned()]
        } else {
            entries
                .iter()
                .map(|record| record.info.browser_message())
                .collect()
        };
        let mut layout = notification_browser_layout(
            viewport,
            self.notification_position,
            &texts.iter().map(String::as_str).collect::<Vec<_>>(),
            selected,
            browser.expanded,
            browser.first,
        )?;
        if !entries.is_empty()
            && let Some(entry) = layout
                .entries
                .iter_mut()
                .find(|entry| entry.index == selected)
        {
            // Keep both one-column viewport margins without changing width or wrapping.
            let shift = viewport.cols.saturating_sub(layout.width + 2).min(2);
            if self.notification_position.is_left() {
                entry.x += shift;
            } else {
                entry.x -= shift;
            }
        }
        Some(BrowserFrame {
            layout,
            ids: entries.iter().map(|record| record.info.id).collect(),
            selected,
        })
    }

    /// The viewport a client's keyboard action lays the browser out against - the same one its
    /// mouse events are hit-tested in.
    pub(super) fn notification_browser_viewport(&self, client_id: ClientId) -> Option<Viewport> {
        self.active_tab_ids
            .get(&client_id)
            .copied()
            .and_then(|tab_id| self.session_surface_for_tab(tab_id))
    }

    /// Remember where the window ended up, so the next move only scrolls if it has to.
    fn settle_notification_browser_window(&mut self, client_id: ClientId) {
        let first = self
            .notification_browser_viewport(client_id)
            .and_then(|viewport| self.notification_browser_frame(client_id, viewport))
            .map(|frame| frame.layout.first);
        if let (Some(first), Some(browser)) =
            (first, self.notification_browsers.get_mut(&client_id))
        {
            browser.first = first;
        }
    }

    /// The cells `client_id`'s browser paints into `viewport`. Empty for a client not browsing.
    pub(super) fn notification_browser_rects(
        &self,
        client_id: ClientId,
        viewport: Viewport,
    ) -> Vec<CellRect> {
        self.notification_browser_frame(client_id, viewport)
            .map(|frame| {
                frame
                    .layout
                    .entries
                    .iter()
                    .map(|entry| CellRect {
                        x: entry.x,
                        y: entry.y,
                        cols: frame.layout.width,
                        rows: notification_height(entry.lines.len()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Paint `client_id`'s browser. The caller has already cleared the floating pane stack, as it
    /// does for the toasts.
    pub(super) fn render_notification_browser(
        &self,
        output: &mut Output,
        viewport: Viewport,
        client_id: ClientId,
    ) -> Result<()> {
        let Some(frame) = self.notification_browser_frame(client_id, viewport) else {
            return Ok(());
        };
        for entry in &frame.layout.entries {
            let declaration = if !frame.ids.is_empty() && entry.index == frame.selected {
                let mut declaration = self.style.colors.text_selected;
                declaration.emphasis_0 = self.style.colors.frame_selected.base;
                declaration
            } else {
                self.style.colors.text_unselected
            };
            let chunks = notification_box_chunks(
                &entry.lines,
                None,
                entry.x,
                entry.y,
                frame.layout.width,
                declaration,
                1.0,
            );
            output.add_character_chunks_to_multiple_clients(
                chunks,
                std::iter::once(client_id),
                None,
            )?;
        }
        Ok(())
    }

    /// Move the selection one entry in `direction` as the list appears on screen: towards older
    /// notifications is down from a top corner and up from a bottom one. Returns whether anything
    /// changed.
    pub(super) fn move_notification_selection(
        &mut self,
        direction: Direction,
        client_id: ClientId,
    ) -> bool {
        let ids: Vec<NotificationId> = self
            .browser_entries()
            .iter()
            .map(|record| record.info.id)
            .collect();
        let towards_older = match direction {
            Direction::Down => self.notification_position.is_top(),
            Direction::Up => !self.notification_position.is_top(),
            Direction::Left | Direction::Right => return false,
        };
        let Some(browser) = self.notification_browsers.get_mut(&client_id) else {
            return false;
        };
        if ids.is_empty() {
            return false;
        }
        let current = browser
            .selected
            .and_then(|id| ids.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        let next = if towards_older {
            (current + 1).min(ids.len() - 1)
        } else {
            current.saturating_sub(1)
        };
        if next == current {
            return false;
        }
        browser.selected = Some(ids[next]);
        browser.expanded = false;
        self.settle_notification_browser_window(client_id);
        true
    }

    /// Show or hide the full text of the selected entry. Returns whether anything changed.
    pub(super) fn toggle_notification_detail(&mut self, client_id: ClientId) -> bool {
        let Some(browser) = self.notification_browsers.get_mut(&client_id) else {
            return false;
        };
        browser.expanded = !browser.expanded;
        self.settle_notification_browser_window(client_id);
        true
    }

    /// Activate the selected entry, as a click on its toast would, and leave the mode - with or
    /// without anything to activate, since Enter always means "done browsing".
    pub(super) fn activate_selected_notification(&mut self, client_id: ClientId, now: Instant) {
        let Some(browser) = self.notification_browsers.get(&client_id) else {
            return;
        };
        let entries = self.browser_entries();
        let target = entries
            .get(Self::browser_selected_index(browser, &entries))
            .map(|record| (record.info.id, record.pane_id, record.on_click.clone()));
        self.leave_notification_browser(client_id);
        if let Some((id, pane_id, on_click)) = target {
            self.activate_notification(id, pane_id, on_click, client_id, now);
        }
    }

    pub(super) fn activate_last_notification(&mut self, client_id: ClientId, now: Instant) {
        let target = self
            .pending_notification()
            .map(|record| (record.info.id, record.pane_id, record.on_click.clone()));
        if self.notification_browsers.contains_key(&client_id) {
            self.leave_notification_browser(client_id);
        }
        if let Some((id, pane_id, on_click)) = target {
            self.activate_notification(id, pane_id, on_click, client_id, now);
        }
    }

    /// Close the browser now and ask the server to put the client back in its base mode.
    ///
    /// Closed here rather than waiting for the mode change to come back round, so the frame the
    /// activation renders already shows the toasts; the `change_mode` that follows finds it gone.
    /// Routed through the server for the reason `sync_scroll_mode_on_focus` gives: the server
    /// holds the mode keys resolve against.
    fn leave_notification_browser(&mut self, client_id: ClientId) {
        self.close_notification_browser(client_id);
        let base_mode = self
            .mode_info
            .get(&client_id)
            .and_then(|mode_info| mode_info.base_mode)
            .unwrap_or_else(|| self.base_input_mode());
        self.bus
            .senders
            .send_to_server(ServerInstruction::ChangeMode(client_id, base_mode, None))
            .with_context(|| format!("failed to leave notifications mode for client {client_id}"))
            .non_fatal();
    }

    /// A mouse event while `client_id` is browsing. Returns whether the browser consumed it.
    ///
    /// Anything over the list belongs to the list: a left press activates the entry under it, the
    /// wheel moves the selection, and every other event is swallowed rather than reaching the pane
    /// hidden underneath. Events elsewhere fall through, and the mode stays.
    pub(super) fn handle_notification_browser_mouse(
        &mut self,
        event: &MouseEvent,
        viewport: Viewport,
        client_id: ClientId,
        now: Instant,
    ) -> bool {
        let Some(frame) = self.notification_browser_frame(client_id, viewport) else {
            return false;
        };
        let Ok(line) = usize::try_from(event.position.line()) else {
            return false;
        };
        let column = event.position.column();
        let Some(hit) = frame.layout.entries.iter().find(|entry| {
            (entry.x..entry.x + frame.layout.width).contains(&column)
                && (entry.y..entry.y + notification_height(entry.lines.len())).contains(&line)
        }) else {
            return false;
        };

        if event.wheel_up || event.wheel_down {
            let direction = if event.wheel_up {
                Direction::Up
            } else {
                Direction::Down
            };
            if self.move_notification_selection(direction, client_id) {
                self.render().non_fatal();
            }
            return true;
        }
        let is_plain_left_press = event.event_type == MouseEventType::Press
            && event.left
            && !event.right
            && !event.middle
            && !event.shift
            && !event.alt
            && !event.ctrl;
        if is_plain_left_press {
            if let Some(id) = frame.ids.get(hit.index).copied()
                && let Some(browser) = self.notification_browsers.get_mut(&client_id)
            {
                browser.selected = Some(id);
            }
            self.activate_selected_notification(client_id, now);
        }
        true
    }
}

use yinz_runtime::errors::prelude::*;

use super::{
    PendingForward, PendingForwardEntry, Screen, ScreenInstruction, ThemePolicy,
    SERVER_CLIPBOARD_FORWARD_TIMEOUT_MS, SERVER_FORWARD_TIMEOUT_MS, STARTUP_SENTINEL_TOKEN,
};
use crate::panes::PaneId;
use crate::pty_writer::PtyWriteInstruction;
use crate::ServerInstruction;

use yinz_types::style::detect_theme_hue;
use yinz_types::style::{HostTerminalThemeMode, ThemeHue};

impl Screen {
    /// Enqueue a whitelisted host-terminal query from pane `pane_id`.
    /// Queries are serialized globally: at most one is in flight to the
    /// client at a time; the rest wait in `forward_queue`. Returns the
    /// allocated token so callers (tests) can observe it if useful.
    pub fn forward_host_query(
        &mut self,
        pane_id: PaneId,
        query: crate::host_query::HostQuery,
    ) -> u32 {
        // ColorPaletteMode is answered locally — Yinz already knows
        // the host's mode from its own startup query and from
        // unsolicited DSR 997 updates. Skip the entire forwarding
        // machinery (no token, no in-flight slot, no host round-trip)
        // and write the reply straight to the originating pane's pty.
        if let crate::host_query::HostQuery::ColorPaletteMode = query {
            self.answer_color_palette_mode_query_locally(pane_id);
            return STARTUP_SENTINEL_TOKEN; // sentinel: no real forward happened
        }
        if let crate::host_query::HostQuery::ClipboardContent { .. } = query {
            if !self.paste_buffer_read_enabled {
                let _ = self.resume_pane_after_forward(pane_id, Vec::new());
                return STARTUP_SENTINEL_TOKEN;
            }
            return self.enqueue_clipboard_forward(pane_id, query);
        }
        let token = self.next_forward_token;
        // Skip over the reserved sentinel (0) on wrap; allocate a fresh
        // u32 for every forward.
        self.next_forward_token = self.next_forward_token.wrapping_add(1);
        if self.next_forward_token == STARTUP_SENTINEL_TOKEN {
            self.next_forward_token = 1;
        }
        if self.forward_in_flight_token.is_some() {
            self.forward_queue.push_back(PendingForward {
                token,
                pane_id,
                query,
            });
        } else {
            self.dispatch_forward(token, pane_id, query);
        }
        token
    }

    fn enqueue_clipboard_forward(
        &mut self,
        pane_id: PaneId,
        query: crate::host_query::HostQuery,
    ) -> u32 {
        let token = self.next_forward_token;
        self.next_forward_token = self.next_forward_token.wrapping_add(1);
        if self.next_forward_token == STARTUP_SENTINEL_TOKEN {
            self.next_forward_token = 1;
        }
        if self.clipboard_forward_in_flight_token.is_some() {
            self.clipboard_forward_queue.push_back(PendingForward {
                token,
                pane_id,
                query,
            });
        } else {
            self.dispatch_clipboard_forward(token, pane_id, query);
        }
        token
    }

    fn dispatch_clipboard_forward(
        &mut self,
        token: u32,
        pane_id: PaneId,
        query: crate::host_query::HostQuery,
    ) {
        let query_bytes = query.to_query_bytes();
        self.pending_clipboard_forwards
            .insert(token, PendingForwardEntry { pane_id, query });
        self.clipboard_forward_in_flight_token = Some(token);
        let _ = self
            .bus
            .senders
            .send_to_server(ServerInstruction::ForwardQueryToHost(
                token,
                query_bytes,
                true,
            ));
        let senders = self.bus.senders.clone();
        crate::global_async_runtime::get_tokio_runtime().spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(
                SERVER_CLIPBOARD_FORWARD_TIMEOUT_MS,
            ))
            .await;
            let _ = senders.send_to_screen(ScreenInstruction::ForwardedReplyFromHost {
                token,
                reply_bytes: Vec::new(),
            });
        });
    }

    fn handle_clipboard_reply(&mut self, token: u32, reply_bytes: Vec<u8>) -> Result<()> {
        if let Some(PendingForwardEntry { pane_id, query }) =
            self.pending_clipboard_forwards.remove(&token)
        {
            let payload = if reply_bytes.is_empty() {
                query.empty_reply_bytes()
            } else {
                reply_bytes
            };
            self.resume_pane_after_forward(pane_id, payload)?;
        }
        self.clipboard_forward_in_flight_token = None;
        while let Some(next) = self.clipboard_forward_queue.pop_front() {
            if !self.pane_exists(&next.pane_id) {
                continue;
            }
            self.dispatch_clipboard_forward(next.token, next.pane_id, next.query);
            break;
        }
        Ok(())
    }

    fn pane_exists(&self, pane_id: &PaneId) -> bool {
        self.tab_containing_pane(*pane_id).is_some()
    }

    fn answer_color_palette_mode_query_locally(&mut self, pane_id: PaneId) {
        if matches!(pane_id, PaneId::Plugin(_)) {
            return;
        }
        let code: u8 = match self.effective_host_terminal_theme_mode() {
            HostTerminalThemeMode::Dark => 1,
            HostTerminalThemeMode::Light => 2,
        };
        let reply = format!("\u{1b}[?997;{}n", code).into_bytes();
        // Route via Tab so the reply lands on the pane in the correct
        // stream position and any PTY input the app emitted while
        // waiting is replayed.
        let _ = self.resume_pane_after_forward(pane_id, reply);
    }

    /// The mode apps and nested guests are told the session is in: a pinned
    /// hue first (`explicit_theme_hue` or a manual `SetDarkTheme` /
    /// `SetLightTheme`), then the mode the session resolved to, and only
    /// when neither is known a guess from the active palette's background.
    pub(super) fn effective_host_terminal_theme_mode(&self) -> HostTerminalThemeMode {
        if let ThemePolicy::Pinned(mode) = self.theme_policy {
            return mode;
        }
        self.host_terminal_theme_mode.unwrap_or_else(|| {
            match detect_theme_hue(self.style.colors.text_unselected.background) {
                ThemeHue::Dark => HostTerminalThemeMode::Dark,
                ThemeHue::Light => HostTerminalThemeMode::Light,
            }
        })
    }

    /// Dispatch a forward to the client and mark the slot as in-flight.
    /// Spawns a one-shot timeout task on the global tokio runtime that
    /// synthesizes an empty reply for `token` after
    /// `SERVER_FORWARD_TIMEOUT_MS`. The handler's token-equality guard
    /// makes the timeout a no-op if a real reply arrived first, so no
    /// explicit cancellation is needed.
    fn dispatch_forward(
        &mut self,
        token: u32,
        pane_id: PaneId,
        query: crate::host_query::HostQuery,
    ) {
        let query_bytes = query.to_query_bytes();
        self.pending_forwarded_queries
            .insert(token, PendingForwardEntry { pane_id, query });
        self.forward_in_flight_token = Some(token);
        let _ = self
            .bus
            .senders
            .send_to_server(ServerInstruction::ForwardQueryToHost(
                token,
                query_bytes,
                false,
            ));
        let senders = self.bus.senders.clone();
        crate::global_async_runtime::get_tokio_runtime().spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(SERVER_FORWARD_TIMEOUT_MS)).await;
            let _ = senders.send_to_screen(ScreenInstruction::ForwardedReplyFromHost {
                token,
                reply_bytes: Vec::new(),
            });
        });
    }

    /// Handle a host-reply observed by the client for token `token`.
    /// Writes the bytes to the originating pane's pty (if still present)
    /// and releases the in-flight slot so the next queued forward can
    /// dispatch.
    ///
    /// An empty `reply_bytes` is treated as "nobody answered" — either
    /// because no client was attached to forward the query, or the
    /// chosen client's host timed out. In that case we try to answer
    /// from Yinz's cached view of the host (pixel dims, bg/fg,
    /// palette) so the pane still gets a well-formed reply instead of
    /// zero bytes. If the cache has nothing relevant either, the pane
    /// receives an empty write and the app decides what to do.
    pub fn handle_forwarded_reply_from_host(
        &mut self,
        token: u32,
        reply_bytes: Vec<u8>,
    ) -> Result<()> {
        if self.clipboard_forward_in_flight_token == Some(token) {
            return self.handle_clipboard_reply(token, reply_bytes);
        }
        if self.pending_clipboard_forwards.contains_key(&token) {
            log::debug!(
                "Dropping stale clipboard reply (token={}, len={} bytes)",
                token,
                reply_bytes.len(),
            );
            return Ok(());
        }
        // Stale-reply guard. Both the real `ForwardedReplyFromHost`
        // path and the server-side timeout path land here. If a real
        // reply landed first (releasing the slot AND dispatching the
        // next queued forward), the late timeout's empty reply for the
        // already-cleared token must be a no-op — releasing the slot
        // again would clobber the new in-flight forward. The same
        // logic also rejects duplicate replies and replies for tokens
        // belonging to a previously-closed pane.
        if self.forward_in_flight_token != Some(token) {
            log::debug!(
                "Dropping stale forwarded reply (token={}, in-flight={:?}, len={} bytes)",
                token,
                self.forward_in_flight_token,
                reply_bytes.len(),
            );
            return Ok(());
        }
        if let Some(entry) = self.pending_forwarded_queries.remove(&token) {
            let PendingForwardEntry { pane_id, query } = entry;
            match pane_id {
                PaneId::Terminal(_) => {
                    let payload = if reply_bytes.is_empty() {
                        self.synthesize_cached_reply(&query)
                    } else {
                        reply_bytes
                    };
                    if let Some(pane) = self
                        .tab_containing_pane_mut(pane_id)
                        .and_then(|tab| tab.get_pane_with_id_mut(pane_id))
                        .and_then(|pane| pane.terminal_io_mut())
                    {
                        pane.record_terminal_color_reply(&query, &payload);
                    }
                    // Route via Tab so the reply lands on the pane in
                    // the correct stream position and any PTY input
                    // the app emitted while waiting is replayed.
                    self.resume_pane_after_forward(pane_id, payload)?;
                },
                PaneId::Plugin(_) => {
                    // Plugin panes do not issue whitelisted host queries;
                    // if we reach here the mapping was populated
                    // erroneously. Drop the reply.
                    log::warn!(
                        "Discarding host reply for plugin pane (token={}); plugins do not forward CSI/OSC queries",
                        token
                    );
                },
            }
        }
        // Release the slot and dispatch the next queued forward, if any.
        self.forward_in_flight_token = None;
        if let Some(next) = self.forward_queue.pop_front() {
            self.dispatch_forward(next.token, next.pane_id, next.query);
        }
        Ok(())
    }

    /// Deliver a forwarded reply (or cache-fallback synthesis, or a
    /// locally-answered query payload) to the originating pane via
    /// the owning Tab. The Tab handler writes the bytes to PTY and
    /// then re-feeds any PTY input that was buffered while the pane
    /// was forward-paused, preserving query/reply ordering.
    pub fn resume_pane_after_forward(
        &mut self,
        pane_id: PaneId,
        reply_bytes: Vec<u8>,
    ) -> Result<()> {
        let terminal_id = match pane_id {
            PaneId::Terminal(id) => id,
            PaneId::Plugin(_) => {
                // Plugin panes do not forward queries — dropping is
                // the correct behaviour matching the existing
                // forwarding path's plugin-pane guard.
                return Ok(());
            },
        };
        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
            tab.resume_pane_after_forward(terminal_id, reply_bytes)
                .non_fatal();
        } else {
            // No tab owns the pane — the pane was closed between the
            // forward dispatch and the reply arrival. The terminal id
            // is probably defunct; the write goes to the PTY writer
            // anyway because an empty payload is the canonical "host
            // declined" reply some apps rely on, and a non-empty
            // payload may still be deliverable until the pty drains.
            let _ = self
                .bus
                .senders
                .send_to_pty_writer(PtyWriteInstruction::Write(reply_bytes, terminal_id, None));
            log::debug!(
                "ResumePaneAfterForward: pane {:?} not in any tab, fell back to direct PTY write",
                pane_id
            );
        }
        Ok(())
    }
}

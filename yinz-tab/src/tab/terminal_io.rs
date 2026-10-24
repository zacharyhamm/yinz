//! PTY output buffering and terminal input delivery.

use super::BufferedTabInstruction;
use super::Tab;
use super::MAX_PENDING_VTE_EVENTS;
use crate::panes::PaneId;
use crate::plugins::PluginId;
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::pty::VteBytes;
use crate::pty_writer::PtyWriteInstruction;
use crate::route::NotificationEnd;
use crate::screen::GuestModalOutcome;
use crate::screen::ScreenInstruction;
use crate::ui::loading_indication::LoadingIndication;
use crate::ClientId;
use std::time::Duration;
use std::time::Instant;
use yinz_panes::pane::AdjustedInput;
use yinz_panes::resize_pty;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::Event;
use yinz_types::internal_ids::TerminalId;
use yinz_types::keys::KeyWithModifier;

impl Tab {
    pub fn handle_pty_bytes(&mut self, pid: TerminalId, bytes: VteBytes) -> Result<()> {
        if self.is_pending {
            self.pending_instructions
                .push(BufferedTabInstruction::HandlePtyBytes(pid, bytes));
            return Ok(());
        }
        let err_context = || format!("failed to handle pty bytes from fd {pid}");
        let pane_id = PaneId::Terminal(pid);
        // A scrolled pane buffers its vte events, so what the user is reading stays put
        let pane_is_scrolled = self
            .get_pane_with_id(pane_id)
            .is_some_and(|pane| pane.is_scrolled());
        if pane_is_scrolled {
            let pending = self.pending_vte_events.entry(pid).or_default();
            pending.push(bytes);
            // Reset scroll - and process all pending events for this pane
            if pending.len() >= MAX_PENDING_VTE_EVENTS {
                if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                    pane.clear_scroll();
                }
                self.process_pending_vte_events(pid)
                    .with_context(err_context)?;
            }
            return Ok(());
        }
        self.process_pty_bytes(pid, bytes).with_context(err_context)
    }
    pub fn remote_pane_ready(&mut self, pid: TerminalId) -> Result<()> {
        // Anything buffered for this pane is loading-screen progress the clear
        // below erases. A user who scrolled the loading pane left it marked
        // scrolled with those frames queued; replaying them later would paint the
        // loading screen over the live terminal, and leaving the scroll set would
        // keep the command's own output buffering behind it.
        self.pending_vte_events.remove(&pid);
        if let Some(pane) = self.get_pane_with_id_mut(PaneId::Terminal(pid)) {
            pane.clear_scroll();
        }
        self.pids_waiting_resize.insert(pid);
        self.process_pty_bytes(pid, b"\x1b[3J\x1b[2J\x1b[H".to_vec())
    }
    pub fn handle_plugin_bytes(
        &mut self,
        pid: PluginId,
        client_id: ClientId,
        bytes: VteBytes,
    ) -> Result<()> {
        if let Some(plugin_pane) = self.get_pane_with_id_mut(PaneId::Plugin(pid)) {
            let mut loading = LoadingIndication::default();
            loading.ended = true;
            plugin_pane.update_loading_indication(loading);
            plugin_pane.handle_plugin_bytes(client_id, bytes);
        }
        Ok(())
    }
    pub fn process_pending_vte_events(&mut self, pid: TerminalId) -> Result<()> {
        if let Some(pending_vte_events) = self.pending_vte_events.get_mut(&pid) {
            let vte_events: Vec<VteBytes> = std::mem::take(pending_vte_events);
            for vte_event in vte_events {
                self.process_pty_bytes(pid, vte_event)
                    .context("failed to process pending vte events")?;
            }
        }
        Ok(())
    }
    /// Deliver a forwarded host reply (or cache-fallback synthesis,
    /// or a locally-answered query payload) to a pane that is currently
    /// forward-paused. The reply bytes are written to the pane's PTY
    /// first (so the app reads them on its stdin in the same stream
    /// position the original query occupied), then the pause flag is
    /// cleared and any PTY bytes that arrived while the pane was
    /// paused are re-fed through vte. The re-feed itself may produce
    /// another forward and re-arm the pause; the cycle bounds at the
    /// buffered byte count.
    pub fn resume_pane_after_forward(
        &mut self,
        pid: TerminalId,
        reply_bytes: Vec<u8>,
    ) -> Result<()> {
        let err_context = || format!("failed to resume pane {pid} after forward");
        // Forwarded replies can teach a color without producing any buffered
        // output. Retain that observation before letting the app use the reply.
        let updates = self
            .get_pane_with_id_mut(PaneId::Terminal(pid))
            .map(|pane| pane.drain_agent_theme_updates())
            .unwrap_or_default();
        for event in updates {
            self.senders
                .send_to_pty_writer(PtyWriteInstruction::RetainAgentTheme(pid, event))
                .context("send learned agent palette to supervisor")
                .non_fatal();
        }
        // Write the reply first so the app sees it on stdin before
        // any subsequent (sync or async) reply that follows in the
        // buffered byte stream.
        if !reply_bytes.is_empty() {
            self.write_to_pane_id_without_preprocessing(reply_bytes, PaneId::Terminal(pid))
                .with_context(err_context)?;
        }
        // Clear the pause and pull out any bytes that were buffered
        // while it waited for the host reply.
        let buffered =
            if let Some(terminal_output) = self.get_pane_with_id_mut(PaneId::Terminal(pid)) {
                terminal_output.clear_forward_pause();
                terminal_output.drain_pending_pty_input()
            } else {
                // Pane closed between forward dispatch and reply arrival.
                return Ok(());
            };
        if !buffered.is_empty() {
            self.process_pty_bytes(pid, buffered)
                .with_context(err_context)?;
        }
        Ok(())
    }
    fn process_pty_bytes(&mut self, pid: TerminalId, bytes: VteBytes) -> Result<()> {
        let err_context = || format!("failed to process pty bytes from pid {pid}");

        // Spelled out rather than `get_pane_with_id_mut`: the pane borrow has to live alongside
        // `self.senders` and `self.character_cell_size` for `resize_pty!` below
        if let Some(terminal_output) = self
            .tiled_panes
            .get_pane_mut(PaneId::Terminal(pid))
            .or_else(|| self.floating_panes.get_pane_mut(PaneId::Terminal(pid)))
            .or_else(|| self.overlay_panes.get_pane_mut(PaneId::Terminal(pid)))
            .or_else(|| {
                self.suppressed_panes
                    .get_mut(&PaneId::Terminal(pid))
                    .map(|s_p| &mut s_p.pane)
            })
        {
            if self.pids_waiting_resize.remove(&pid) {
                resize_pty!(
                    terminal_output,
                    self.os_api,
                    self.senders,
                    self.character_cell_size
                )
                .with_context(err_context)?;
            }
            terminal_output.handle_pty_bytes(bytes);
            let messages_to_pty = terminal_output.drain_messages_to_pty();
            let agent_theme_updates = terminal_output.drain_agent_theme_updates();
            let forwarded_queries = terminal_output.drain_forwarded_queries();
            // If at least one forward was produced, the pane stopped
            // processing its `pending_pty_input` queue at the byte
            // that produced the forward. Arm the pause so subsequent
            // PTY bytes accumulate in the same queue rather than
            // being processed; they will be drained in stream order
            // when the host reply (or cache-fallback synthesis, or
            // 500 ms timeout) lands via `resume_pane_after_forward`.
            // Pause arming is independent of which forward bears the
            // reply: resume is keyed by pane_id, not by token, so a
            // single flag suffices for any number of queued forwards.
            if !forwarded_queries.is_empty() {
                terminal_output.arm_forward_pause();
            }
            let nested_session_messages = terminal_output.drain_nested_session_messages();
            let clipboard_update = terminal_output.drain_clipboard_update();
            let desktop_notifications = terminal_output.drain_desktop_notifications();
            let osc7_cwd = terminal_output.drain_osc7_cwd();
            for event in agent_theme_updates {
                self.senders
                    .send_to_pty_writer(PtyWriteInstruction::RetainAgentTheme(pid, event))
                    .context("send learned agent palette to supervisor")
                    .non_fatal();
            }
            for message in messages_to_pty {
                self.write_to_pane_id_without_preprocessing(message, PaneId::Terminal(pid))
                    .with_context(err_context)?;
            }
            for query in forwarded_queries {
                let _ = self
                    .senders
                    .send_to_screen(ScreenInstruction::ForwardHostQuery {
                        pane_id: PaneId::Terminal(pid),
                        query,
                    });
            }
            for message in nested_session_messages {
                let _ =
                    self.senders
                        .send_to_screen(ScreenInstruction::NestedSessionMessageFromPane {
                            pane_id: PaneId::Terminal(pid),
                            message,
                        });
            }
            if let Some(string) = clipboard_update {
                self.write_selection_to_clipboard(&string)
                    .with_context(err_context)?;
            }
            if !desktop_notifications.is_empty() {
                // The screen knows which protocol each client's host speaks.
                self.senders
                    .send_to_screen(ScreenInstruction::ForwardDesktopNotifications {
                        pane_id: pid,
                        notifications: desktop_notifications,
                    })
                    .with_context(err_context)?;
            }
            if let Some(path) = osc7_cwd {
                let _ = self
                    .senders
                    .send_to_pty(PtyInstruction::NotifyCwdFromOsc7(pid, path));
            }
        }
        Ok(())
    }

    pub fn write_to_terminals_on_current_tab(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
        client_id: ClientId,
    ) -> Result<bool> {
        // returns true if a UI update should be triggered (eg. when closing a command pane with
        // ctrl-c)
        let mut should_trigger_ui_change = false;
        let mut pane_ids = self.get_static_and_floating_pane_ids();
        pane_ids.extend(
            self.stack_list_of_member
                .keys()
                .filter(|member| self.pane_is_hidden_stack_list_member(member))
                .copied(),
        );
        for pane_id in pane_ids {
            let ui_change_triggered = self
                .write_to_pane_id(
                    key_with_modifier,
                    raw_input_bytes.clone(),
                    raw_input_bytes_are_kitty,
                    pane_id,
                    Some(client_id),
                    None,
                )
                .context("failed to write to terminals on current tab")?;
            if ui_change_triggered {
                should_trigger_ui_change = true;
            }
        }
        Ok(should_trigger_ui_change)
    }

    pub fn write_to_active_terminal(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
        client_id: ClientId,
        completion: Option<NotificationEnd>,
    ) -> Result<bool> {
        // returns true if a UI update should be triggered (eg. if a command pane
        // was closed with ctrl-c)
        let err_context = || {
            format!(
                "failed to write to active terminal for client {client_id} - msg: {raw_input_bytes:?}"
            )
        };

        self.clear_search(client_id);
        self.mouse_help_text_visible.clear();
        let pane_id = self
            .get_active_pane_id(client_id)
            .ok_or_else(|| anyhow!("failed to find active pane id for client {client_id}"))
            .with_context(err_context)?;
        // Can't use 'err_context' here since it borrows 'raw_input_bytes'
        self.write_to_pane_id(
            key_with_modifier,
            raw_input_bytes,
            raw_input_bytes_are_kitty,
            pane_id,
            Some(client_id),
            completion,
        )
        .with_context(|| format!("failed to write to active terminal for client {client_id}"))
    }

    pub fn paste_to_pane_id(
        &mut self,
        bytes: Vec<u8>,
        pane_id: PaneId,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        let bracketed_paste_begin = vec![27, 91, 50, 48, 48, 126];
        let bracketed_paste_end = vec![27, 91, 50, 48, 49, 126];

        self.write_to_pane_id(&None, bracketed_paste_begin, false, pane_id, None, None)?;
        self.write_to_pane_id(&None, bytes, false, pane_id, None, None)?;
        self.write_to_pane_id(&None, bracketed_paste_end, false, pane_id, None, completion)?;
        Ok(())
    }

    pub fn paste_to_active_terminal(
        &mut self,
        bytes: Vec<u8>,
        client_id: ClientId,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        let err_context = || format!("failed to paste to active terminal for client {client_id}");
        let active_pane_id = self
            .get_active_pane_id(client_id)
            .ok_or_else(|| anyhow!("no active pane for client {client_id}"))
            .with_context(err_context)?;
        if self.pane_tabs_input(active_pane_id, &None, Some(client_id))? {
            return Ok(());
        }
        self.paste_to_pane_id(bytes, active_pane_id, completion)
    }

    /// Changes only for input delivered to the terminal, not terminal query replies.
    pub fn agent_input_revision(&self, pane_id: PaneId) -> u64 {
        self.get_pane_with_id(pane_id)
            .map(|pane| pane.agent_input_revision())
            .unwrap_or(0)
    }

    /// When the input counted by [`Tab::agent_input_revision`] last arrived.
    pub fn agent_input_at(&self, pane_id: PaneId) -> Option<Instant> {
        self.get_pane_with_id(pane_id)
            .and_then(|pane| pane.agent_input_at())
    }

    pub fn write_key_to_pane_id(
        &mut self,
        pane_id: PaneId,
        key: KeyWithModifier,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let bytes = yinz_key_codec::keys::serialize_non_kitty(&key)
            .context("key has no terminal encoding")?
            .into_bytes();
        self.write_to_pane_id(&Some(key), bytes, false, pane_id, None, completion_tx)?;
        Ok(())
    }

    pub fn write_to_pane_id(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
        pane_id: PaneId,
        client_id: Option<ClientId>,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<bool> {
        self.write_pane_input(
            key_with_modifier,
            raw_input_bytes,
            raw_input_bytes_are_kitty,
            pane_id,
            client_id,
            completion_tx,
            true,
        )
    }

    pub(super) fn write_mouse_to_pane_id(
        &mut self,
        bytes: Vec<u8>,
        pane_id: PaneId,
        client_id: Option<ClientId>,
    ) -> Result<bool> {
        self.write_pane_input(&None, bytes, false, pane_id, client_id, None, false)
    }

    fn write_pane_input(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
        pane_id: PaneId,
        client_id: Option<ClientId>,
        completion_tx: Option<NotificationEnd>,
        count_input: bool,
    ) -> Result<bool> {
        // returns true if we need to update the UI (eg. when a command pane is closed with ctrl-c)
        if self.pane_tabs_input(pane_id, key_with_modifier, client_id)? {
            return Ok(true);
        }
        // Only client input may wait on the plugin executor: synchronous plugin writes
        // would otherwise wait for a barrier queued behind their own running event.
        let wait_for_plugin_input = completion_tx
            .as_ref()
            .is_some_and(NotificationEnd::requires_input_barrier);
        let err_context = || format!("failed to write to pane with id {pane_id:?}");

        let mut should_update_ui = false;
        let is_sync_panes_active = self.is_sync_panes_active();

        // Spelled out rather than `get_pane_with_id_mut`: the pane borrow spans the
        // `pending_agent_escapes` bookkeeping and the sends below
        let active_pane = self
            .floating_panes
            .get_mut(&pane_id)
            .or_else(|| self.overlay_panes.get_mut(&pane_id))
            .or_else(|| self.tiled_panes.get_pane_mut(pane_id))
            .or_else(|| self.suppressed_panes.get_mut(&pane_id).map(|p| &mut p.pane))
            .ok_or_else(|| anyhow!(format!("failed to find pane with id {pane_id:?}")))
            .with_context(err_context)?;

        // We always write for non-synced terminals.
        // However if the terminal is part of a tab-sync, we need to
        // check if the terminal should receive input or not (depending on its
        // 'exclude_from_sync' configuration).
        let should_not_write_to_terminal = is_sync_panes_active && active_pane.exclude_from_sync();

        if should_not_write_to_terminal {
            return Ok(should_update_ui);
        }

        if self.agent_double_escape
            && matches!(pane_id, PaneId::Terminal(_))
            && let (Some(client_id), Some(key)) = (client_id, key_with_modifier)
        {
            let target = (client_id, pane_id);
            if active_pane.agent_info().is_some()
                && !active_pane.is_held()
                && !active_pane.is_suspended()
                && active_pane.guest_modal_selection(client_id).is_none()
                && key.bare_key == yinz_types::keys::BareKey::Esc
                && key.has_no_modifiers()
            {
                let now = Instant::now();
                if !self
                    .pending_agent_escapes
                    .remove(&target)
                    .is_some_and(|first| now.duration_since(first) < Duration::from_secs(1))
                {
                    self.pending_agent_escapes.insert(target, now);
                    return Ok(true);
                }
                should_update_ui = true;
            } else {
                should_update_ui |= self.pending_agent_escapes.remove(&target).is_some();
            }
        }

        match pane_id {
            PaneId::Terminal(active_terminal_id) => {
                match active_pane.adjust_input_to_terminal(
                    key_with_modifier,
                    raw_input_bytes,
                    raw_input_bytes_are_kitty,
                    client_id,
                ) {
                    Some(AdjustedInput::WriteBytesToTerminal(adjusted_input)) => {
                        let record_input = count_input && !adjusted_input.is_empty();
                        self.senders
                            .send_to_pty_writer(PtyWriteInstruction::Write(
                                adjusted_input,
                                active_terminal_id,
                                completion_tx,
                            ))
                            .with_context(err_context)?;
                        if record_input {
                            active_pane.record_agent_input(key_with_modifier.as_ref().is_some_and(
                                |key| key.is_key_without_modifier(yinz_types::keys::BareKey::Enter),
                            ));
                        }
                    },
                    Some(AdjustedInput::ReRunCommandInThisPane) => {
                        // Through the screen, which re-runs it with
                        // `rerun_terminal_pane_with_id` and reads the agent record around the
                        // restart. The screen's channel is unbounded, so a tab may send to it
                        // from the screen thread.
                        self.senders
                            .send_to_screen(ScreenInstruction::RerunCommandPane(
                                active_terminal_id,
                                completion_tx,
                            ))
                            .with_context(err_context)?;
                        should_update_ui = true;
                    },
                    Some(AdjustedInput::ResumeSuspendedProcessInThisPane) => {
                        self.senders
                            .send_to_pty(PtyInstruction::ResumeSuspendedPane(PaneId::Terminal(
                                active_terminal_id,
                            )))
                            .with_context(err_context)?;
                    },
                    Some(AdjustedInput::CloseThisPane) => {
                        self.senders
                            .send_to_screen(ScreenInstruction::CloseFocusWithPaneId(
                                PaneId::Terminal(active_terminal_id),
                                completion_tx,
                            ))
                            .with_context(err_context)?;
                        should_update_ui = true;
                    },
                    Some(AdjustedInput::DropToShellInThisPane { working_dir }) => {
                        self.pids_waiting_resize.insert(active_terminal_id);
                        self.senders
                            .send_to_pty(PtyInstruction::DropToShellInPane {
                                pane_id: PaneId::Terminal(active_terminal_id),
                                shell: self.default_shell.clone(),
                                working_dir,
                                completion_tx,
                            })
                            .with_context(err_context)?;
                        should_update_ui = true;
                    },
                    Some(AdjustedInput::GuestModalSelectionChanged) => {
                        should_update_ui = true;
                    },
                    Some(AdjustedInput::GuestModalZoom) => {
                        if let Some(client_id) = client_id {
                            let _ =
                                self.senders
                                    .send_to_screen(ScreenInstruction::GuestModalChoice {
                                        client_id,
                                        pane_id: PaneId::Terminal(active_terminal_id),
                                        outcome: GuestModalOutcome::Zoom,
                                    });
                        }
                    },
                    Some(AdjustedInput::GuestModalDescend) => {
                        if let Some(client_id) = client_id {
                            let _ =
                                self.senders
                                    .send_to_screen(ScreenInstruction::GuestModalChoice {
                                        client_id,
                                        pane_id: PaneId::Terminal(active_terminal_id),
                                        outcome: GuestModalOutcome::Descend,
                                    });
                        }
                    },
                    Some(_) => {},
                    None => {},
                }
            },
            PaneId::Plugin(pid) => match active_pane.adjust_input_to_terminal(
                key_with_modifier,
                raw_input_bytes,
                raw_input_bytes_are_kitty,
                client_id,
            ) {
                Some(AdjustedInput::WriteKeyToPlugin(key_with_modifier)) => {
                    self.senders
                        .send_to_plugin(PluginInstruction::Update(vec![(
                            Some(pid),
                            client_id,
                            Event::Key(key_with_modifier),
                        )]))
                        .with_context(err_context)?;
                    if wait_for_plugin_input
                        && active_pane.pane_tabs().is_some()
                        && let Some(completion) = completion_tx
                    {
                        self.senders
                            .send_to_plugin(PluginInstruction::InputBarrier(pid, completion))
                            .with_context(err_context)?;
                    }
                },
                Some(AdjustedInput::WriteBytesToTerminal(adjusted_input)) => {
                    let mut plugin_updates = vec![];
                    for key in yinz_key_codec::keys::parse_keys(&adjusted_input) {
                        plugin_updates.push((Some(pid), client_id, Event::Key(key)));
                    }
                    self.senders
                        .send_to_plugin(PluginInstruction::Update(plugin_updates))
                        .with_context(err_context)?;
                    if wait_for_plugin_input
                        && active_pane.pane_tabs().is_some()
                        && let Some(completion) = completion_tx
                    {
                        self.senders
                            .send_to_plugin(PluginInstruction::InputBarrier(pid, completion))
                            .with_context(err_context)?;
                    }
                },
                Some(_) => {},
                None => {},
            },
        }
        Ok(should_update_ui)
    }
    pub fn write_to_pane_id_without_preprocessing(
        &mut self,
        raw_input_bytes: Vec<u8>,
        pane_id: PaneId,
    ) -> Result<bool> {
        // returns true if we need to update the UI (eg. when a command pane is closed with ctrl-c)
        let err_context = || format!("failed to write to pane with id {pane_id:?}");

        let mut should_update_ui = false;

        match pane_id {
            PaneId::Terminal(active_terminal_id) => {
                self.senders
                    .send_to_pty_writer(PtyWriteInstruction::Write(
                        raw_input_bytes,
                        active_terminal_id,
                        None,
                    ))
                    .with_context(err_context)?;
                should_update_ui = true;
            },
            PaneId::Plugin(_pid) => {
                log::error!("Unsupported plugin action");
            },
        }
        Ok(should_update_ui)
    }
}

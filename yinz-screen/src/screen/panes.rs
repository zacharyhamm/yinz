//! Panes as the pty thread and the CLI see them: bytes arriving for a terminal, a new pane to
//! place, a pane to close, focus or dump.
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TerminalId;

use super::agent_placement::AgentPlacement;
use super::thread_main::{replay_pending_pane_event, PendingEvents};
use super::{
    active_tab_and_connected_client_id, active_tab_and_connected_client_id_with_first_tab_fallback,
    Screen, ScreenInstruction,
};
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty::{ClientTabIndexOrPaneId, PtyInstruction};
use crate::route::NotificationEnd;
use crate::tab::Tab;
use crate::{ClientId, ServerInstruction};
use std::time::Instant;
use yinz_bus::pty::VteBytes;
use yinz_bus::screen::{HoldForCommand, InitialTitle};
use yinz_types::data::{Event, NewPanePlacement};
use yinz_types::geometry::Direction;
use yinz_types::input::layout::Run;

impl Screen {
    /// Focuses a pane by id for a client, answering the completion with an error when the pane
    /// does not exist or is already focused. Deferred while a tab's layout has not landed, for
    /// the reason `GoToTab` is.
    pub(super) fn focus_pane_by_request(
        &mut self,
        pane_id: PaneId,
        should_float_if_hidden: bool,
        should_be_in_place_if_hidden: bool,
        client_id: ClientId,
        mut completion_tx: Option<NotificationEnd>,
        pending: &mut PendingEvents,
    ) -> Result<()> {
        let pane_exists = self.tab_containing_pane(pane_id).is_some();
        if !pane_exists {
            if let Some(c) = completion_tx.as_mut() {
                c.set_exit_status(1);
                c.set_error_message(format!("Pane with id {:?} not found", pane_id));
            }
        } else if self.has_pending_tabs() {
            // Deferred for the reason `GoToTab` is: a tab whose layout has not landed
            // has provisional geometry, and its layout landing may move the client
            // itself. A plugin that spawned a pane into a tab made for it focuses the
            // pane in the same breath, and that focus should land after the layout,
            // on the tab as it really is.
            pending
                .waiting_for_tab
                .push(ScreenInstruction::FocusPaneWithId(
                    pane_id,
                    should_float_if_hidden,
                    should_be_in_place_if_hidden,
                    client_id,
                    completion_tx,
                ));
        } else {
            let already_focused = self
                .get_active_pane_id(&client_id)
                .map(|active| active == pane_id)
                .unwrap_or(false);
            if already_focused {
                if let Some(c) = completion_tx.as_mut() {
                    c.set_exit_status(1);
                    c.set_error_message(format!("Pane {:?} is already focused", pane_id));
                }
            } else {
                self.focus_pane_with_id(
                    pane_id,
                    should_float_if_hidden,
                    should_be_in_place_if_hidden,
                    client_id,
                )?;
                self.clear_bell_for_pane_id(pane_id, client_id);
                self.sync_scroll_mode_on_focus(client_id)?;
                self.log_and_report_session_state()?;
            }
        }
        Ok(())
    }

    /// Closes a pane, whether its command exited on its own (no client, no completion: the
    /// `pane_exit` hook's moment) or an action asked for it. A close for a pane that has not
    /// been placed yet waits in `pending` for it.
    pub(super) fn close_pane_from_instruction(
        &mut self,
        id: PaneId,
        client_id: Option<ClientId>,
        completion_tx: Option<NotificationEnd>,
        exit_status: Option<i32>,
        pending: &mut PendingEvents,
    ) -> Result<()> {
        // A command pane whose command exited without `hold_on_close` arrives here from
        // the pty with no client and no completion; explicit ID-targeted actions carry
        // a completion. A plugin's `close_self` also has neither, but no command. This is the `pane_exit`
        // hook's moment for a pane that is not held.
        if client_id.is_none() && completion_tx.is_none() {
            if let Some(run_command) = self
                .tab_containing_pane(id)
                .and_then(|tab| tab.pane_run_command(id))
            {
                self.fire_pane_exit_hook(id, exit_status, &run_command, false);
            }
        }
        if self.destroyed_panes.contains(&id) && self.tab_containing_pane(id).is_none() {
            drop(completion_tx);
        } else if client_id.is_some() || completion_tx.is_some() {
            if self.tab_containing_pane(id).is_some() {
                self.close_pane_explicitly(id, completion_tx);
            } else {
                pending
                    .waiting_for_pane
                    .entry(id)
                    .or_default()
                    .push(ScreenInstruction::ClosePane(
                        id,
                        client_id,
                        completion_tx,
                        exit_status,
                    ));
            }
        } else {
            self.repaint_tab_of_closing_overlay(id);
            if let Some(tab) = self.tab_containing_pane_mut(id) {
                tab.close_pane(id, false, exit_status);
                if !tab.has_pane_with_pid(&id) {
                    self.cleanup_closed_pane(id);
                }
            } else {
                pending
                    .waiting_for_pane
                    .entry(id)
                    .or_default()
                    .push(ScreenInstruction::ClosePane(id, None, None, exit_status));
            }
            // Natural process exit still owes the PTY its resource cleanup.
            let _ = self
                .bus
                .senders
                .send_to_pty(PtyInstruction::ClosePane(id, None));
        }

        let connected_client_ids: Vec<ClientId> = self.active_tab_ids.keys().copied().collect();
        for connected_client_id in connected_client_ids {
            self.sync_scroll_mode_on_focus(connected_client_id)?;
        }
        Ok(())
    }

    /// Writes a pane's screen (the named one, or the client's active one) to `file`, or sends it
    /// back to the CLI client as log output when no file is named.
    pub(super) fn dump_screen(
        &mut self,
        file: Option<String>,
        client_id: ClientId,
        full: bool,
        pane_id: Option<PaneId>,
        completion_tx: Option<NotificationEnd>,
        cli_client_id: Option<ClientId>,
        ansi: bool,
    ) -> Result<()> {
        match file {
            Some(file_path) => {
                // Write dump to file (existing behavior)
                match pane_id {
                    Some(pane_id) => {
                        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
                            if ansi {
                                tab.dump_with_ansi_terminal_screen(
                                    Some(file_path.clone()),
                                    pane_id,
                                    full,
                                )?;
                            } else {
                                tab.dump_terminal_screen(Some(file_path.clone()), pane_id, full)?;
                            }
                        }
                    },
                    None => {
                        if ansi {
                            active_tab_and_connected_client_id!(
                                self,
                                client_id,
                                |tab: &mut Tab, client_id: ClientId| tab.dump_with_ansi_active_terminal_screen(
                                    Some(file_path.to_string()),
                                    client_id,
                                    full
                                ),
                                ?
                            );
                        } else {
                            active_tab_and_connected_client_id!(
                                self,
                                client_id,
                                |tab: &mut Tab, client_id: ClientId| tab.dump_active_terminal_screen(
                                    Some(file_path.to_string()),
                                    client_id,
                                    full
                                ),
                                ?
                            );
                        }
                    },
                }
                self.render()?;
                drop(completion_tx);
            },
            None => {
                // Dump to STDOUT via Log
                let dump = match pane_id {
                    Some(pane_id) => {
                        let mut result = String::new();
                        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
                            let dump = if ansi {
                                tab.get_dump_with_ansi_terminal_screen(pane_id, full)
                            } else {
                                tab.get_dump_terminal_screen(pane_id, full)
                            };
                            if let Some(dump) = dump {
                                result = dump;
                            }
                        }
                        result
                    },
                    None => {
                        let mut result = String::new();
                        if ansi {
                            active_tab_and_connected_client_id!(
                                self,
                                client_id,
                                |tab: &mut Tab, client_id: ClientId| {
                                    result = tab.get_dump_with_ansi_active_terminal_screen(client_id, full);
                                    Ok::<(), anyhow::Error>(())
                                },
                                ?
                            );
                        } else {
                            active_tab_and_connected_client_id!(
                                self,
                                client_id,
                                |tab: &mut Tab, client_id: ClientId| {
                                    result = tab.get_dump_active_terminal_screen(client_id, full);
                                    Ok::<(), anyhow::Error>(())
                                },
                                ?
                            );
                        }
                        result
                    },
                };
                self.bus.senders.send_to_server(ServerInstruction::Log(
                    vec![dump],
                    cli_client_id.unwrap_or(client_id),
                    completion_tx,
                ))?;
            },
        }
        Ok(())
    }

    /// The pty thread has opened a terminal (or a plugin has been placed): put its pane where
    /// the instruction says - beside a client's active pane, in a given tab, or split off a named
    /// pane - holding it for its command if asked, then replay what was waiting for the pane.
    ///
    /// An agent's pane may be redirected to a roomier tab or one of its own; see
    /// `agent_pane_target`.
    pub(super) fn place_new_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<InitialTitle>,
        hold_for_command: HoldForCommand,
        invoked_with: Option<Run>,
        mut new_pane_placement: NewPanePlacement,
        start_suppressed: bool,
        client_or_tab_index: ClientTabIndexOrPaneId,
        mut completion_tx: Option<NotificationEnd>,
        set_blocking: bool,
        pending: &mut PendingEvents,
    ) -> Result<()> {
        if let Some(c) = completion_tx.as_mut() {
            c.set_affected_pane_id(pid)
        }

        let client_or_tab_index = self.resolve_pane_placement(client_or_tab_index);

        if let Some(c) = completion_tx.as_mut().filter(|_| !set_blocking) {
            c.wait_for_placement();
            if let Some(target) = &c.agent_tab {
                let live = !self.deferred_tab_closes.contains(&target.tab_id)
                    && self.tabs.get(&target.tab_id).is_some_and(|tab| {
                        target
                            .lifetime
                            .ptr_eq(&std::sync::Arc::downgrade(&tab.agent_tab_lifetime))
                    });
                if !live {
                    c.set_error_message("destination tab closed during agent launch".into());
                    pending.waiting_for_pane.remove(&pid);
                    self.forget_unplaced_pane(
                        pid,
                        "destination tab closed during agent launch".into(),
                    );
                    return Ok(());
                }
            }
        }
        // The CLI can spawn before the initial NewTab arrives from the plugin thread.
        if !self.has_created_tab
            && matches!(
                client_or_tab_index,
                ClientTabIndexOrPaneId::ClientId(_) | ClientTabIndexOrPaneId::ClientIdNoFocus(_)
            )
            && completion_tx.as_ref().is_none_or(|c| c.agent_tab.is_none())
        {
            pending.waiting_for_tab.push(ScreenInstruction::NewPane(
                pid,
                initial_pane_title,
                hold_for_command,
                invoked_with,
                new_pane_placement,
                start_suppressed,
                client_or_tab_index,
                completion_tx,
                set_blocking,
            ));
            return Ok(());
        }

        let blocking_notification = if set_blocking {
            completion_tx.take()
        } else {
            None
        };

        // An agent's pane may be sent to a roomier tab than the one it was asked for,
        // or to a tab of its own - `agent_pane_target` says when. Every other pane
        // keeps the target it came with.
        let AgentPlacement {
            target: client_or_tab_index,
            focus_after_placing,
            created_tab_id,
        } = self.agent_pane_target(
            invoked_with.as_ref(),
            &mut new_pane_placement,
            start_suppressed,
            client_or_tab_index,
        );
        // The tab the pane went into, so that holding it for its command below reaches the
        // same tab whichever way it was chosen - including a tab whose layout has not landed,
        // which queues both the placement and the hold.
        if let Some(c) = completion_tx.as_mut() {
            c.created_tab_id = c.created_tab_id.or(created_tab_id);
            let tab_id = match client_or_tab_index {
                ClientTabIndexOrPaneId::TabIndex(id)
                | ClientTabIndexOrPaneId::TabIndexNoFocus(id)
                | ClientTabIndexOrPaneId::TabIndexForClient(id, _) => Some(id),
                ClientTabIndexOrPaneId::ClientId(id)
                | ClientTabIndexOrPaneId::ClientIdNoFocus(id) => {
                    self.tab_a_client_is_looking_at(id)
                },
                ClientTabIndexOrPaneId::PaneId(id)
                | ClientTabIndexOrPaneId::PaneIdOrClient(id, _) => self.tab_id_containing_pane(id),
            };
            if let Some(tab) = tab_id.and_then(|id| self.tabs.get(&id)) {
                c.agent_tab
                    .get_or_insert_with(|| yinz_bus::route::AgentTabPlacement {
                        tab_id: tab.id,
                        lifetime: std::sync::Arc::downgrade(&tab.agent_tab_lifetime),
                        created: c.created_tab_id == Some(tab.id),
                    });
            }
        }
        let mut placed_in: Option<TabId> = None;
        let insertion = (|| -> Result<()> {
            match client_or_tab_index {
                ClientTabIndexOrPaneId::ClientId(client_id)
                | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => {
                    let should_focus_pane =
                        matches!(client_or_tab_index, ClientTabIndexOrPaneId::ClientId(_));
                    active_tab_and_connected_client_id_with_first_tab_fallback!(self, client_id, |tab: &mut Tab, client_id: Option<ClientId>| {
                placed_in = Some(tab.id);
                tab.new_pane(pid,
                   initial_pane_title,
                   invoked_with,
                   start_suppressed,
                   should_focus_pane,
                   new_pane_placement,
                   client_id,
                   blocking_notification
               )
            }, ?);
                    if should_focus_pane {
                        self.sync_scroll_mode_on_focus(client_id)?;
                    }
                    if placed_in.is_none() {
                        self.forget_unplaced_pane(pid, "destination tab no longer exists".into());
                    }
                },
                ClientTabIndexOrPaneId::TabIndex(tab_index)
                | ClientTabIndexOrPaneId::TabIndexNoFocus(tab_index)
                | ClientTabIndexOrPaneId::TabIndexForClient(tab_index, _) => {
                    let requesting_client = match client_or_tab_index {
                        ClientTabIndexOrPaneId::TabIndexForClient(_, client) => Some(client),
                        _ => None,
                    };
                    let should_focus_pane = requesting_client.map_or_else(
                        || matches!(client_or_tab_index, ClientTabIndexOrPaneId::TabIndex(_)),
                        |client| self.active_tab_ids.get(&client) == Some(&tab_index),
                    );
                    // Some placements (directional split, stacked without a
                    // target pane) need a client_id to know which pane to
                    // split relative to. Only resolve one when required.
                    let needs_client_id = matches!(
                        new_pane_placement,
                        NewPanePlacement::Tiled {
                            direction: Some(_),
                            ..
                        } | NewPanePlacement::Stacked {
                            pane_id_to_stack_under: None,
                            ..
                        }
                    );
                    let client_id = if requesting_client.is_some() {
                        requesting_client
                    } else if needs_client_id {
                        self.active_tab_ids
                            .iter()
                            .find(|(_, tid)| **tid == tab_index)
                            .map(|(cid, _)| *cid)
                            .or_else(|| self.active_tab_ids.keys().next().copied())
                    } else {
                        None
                    };
                    if let Some(active_tab) = self.tabs.get_mut(&tab_index) {
                        active_tab.new_pane(
                            pid,
                            initial_pane_title,
                            invoked_with,
                            start_suppressed,
                            should_focus_pane,
                            new_pane_placement,
                            client_id,
                            blocking_notification,
                        )?;
                        placed_in = Some(tab_index);
                    } else {
                        self.forget_unplaced_pane(
                            pid,
                            format!("tab index {tab_index:?} not found"),
                        );
                    }
                },
                ClientTabIndexOrPaneId::PaneId(pane_id)
                | ClientTabIndexOrPaneId::PaneIdOrClient(pane_id, _) => {
                    // The second spelling cannot reach here - it was resolved above - but
                    // an arm for it keeps the match exhaustive without a catch-all.
                    let mut found = false;
                    let all_tabs = self.get_tabs_mut();
                    let should_focus_pane = false;
                    for tab in all_tabs.values_mut() {
                        if tab.has_pane_with_pid(&pane_id) {
                            match new_pane_placement {
                                NewPanePlacement::Tiled {
                                    direction: Some(direction),
                                    borderless,
                                } => {
                                    if direction == Direction::Left || direction == Direction::Right
                                    {
                                        tab.vertical_split_of_pane_id(
                                            pid,
                                            initial_pane_title,
                                            invoked_with,
                                            pane_id,
                                            blocking_notification,
                                            borderless,
                                        )?;
                                    } else {
                                        tab.horizontal_split_of_pane_id(
                                            pid,
                                            initial_pane_title,
                                            invoked_with,
                                            pane_id,
                                            blocking_notification,
                                            borderless,
                                        )?;
                                    }
                                },
                                _ => {
                                    tab.new_pane(
                                        pid,
                                        initial_pane_title,
                                        invoked_with,
                                        start_suppressed,
                                        should_focus_pane,
                                        new_pane_placement,
                                        None,
                                        blocking_notification,
                                    )?;
                                },
                            }
                            placed_in = Some(tab.id);
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        self.forget_unplaced_pane(pid, format!("no tab contains pane {pane_id:?}"));
                    }
                },
            };
            Ok(())
        })();
        if let Err(error) = insertion {
            if let Some(completion) = completion_tx.as_mut() {
                completion.set_error_message(format!("cannot place agent pane: {error:#}"));
            }
            if !self.tabs.values().any(|tab| tab.has_pane_with_pid(&pid)) {
                self.forget_unplaced_pane(pid, format!("cannot place pane: {error:#}"));
            }
            return Err(error);
        }
        if let Some(completion) = completion_tx.take() {
            if let Some(tab) = placed_in.and_then(|id| self.tabs.get_mut(&id)) {
                if tab.has_pane_with_pid(&pid) {
                    tab.complete_agent_placement(pid, completion);
                } else {
                    self.forget_unplaced_pane(pid, "destination could not accept the pane".into());
                }
            }
        }
        if let (Some(hold_for_command), Some(tab_id)) = (hold_for_command, placed_in) {
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                let is_first_run = true;
                tab.hold_pane(pid, None, is_first_run, hold_for_command);
            }
        }
        // A `TabIndex` target places without a client, so a pane redirected to a tab
        // that already exists is brought to its client here. A tab made for the pane
        // needs nothing: `apply_layout` moves the client when the layout lands.
        if let Some(client_id) = focus_after_placing {
            // The pane is placed by now; a focus that misses is not a reason to skip
            // the hooks, the report and the render below.
            self.focus_pane_with_id(pid, false, false, client_id)
                .context("failed to focus agent pane after placing it in another tab")
                .non_fatal();
            self.sync_scroll_mode_on_focus(client_id)
                .context("failed to synchronize scroll mode after focusing an agent pane")
                .non_fatal();
        }
        // Before the replay: a pane that exited before the screen created it starts,
        // and then exits, in that order.
        self.fire_agent_start_for_new_pane(pid);
        if let Some(pending_events) = pending.waiting_for_pane.remove(&pid) {
            for event in pending_events {
                replay_pending_pane_event(self, event, pending).non_fatal();
            }
        }
        Ok(())
    }

    /// Bytes arrived from a terminal: feed them to the pane that owns it, forwarding a copy to
    /// every plugin subscribed to that command's output. Bytes for a pane that does not exist yet
    /// wait in `pending` for it.
    pub(super) fn handle_pty_bytes(
        &mut self,
        pid: TerminalId,
        vte_bytes: VteBytes,
        pending: &mut PendingEvents,
    ) -> Result<()> {
        self.mark_subscriber_pane_dirty(PaneId::Terminal(pid));
        self.pane_output_activity
            .insert(PaneId::Terminal(pid), Instant::now());
        let mut vte_bytes = Some(vte_bytes);
        {
            for tab in self.tabs.values_mut() {
                if tab.has_terminal_pid(pid) {
                    if let Some(bytes) = vte_bytes.take() {
                        if let Some(yinz_types::input::layout::Run::Command(command)) = tab
                            .get_pane_with_id(PaneId::Terminal(pid))
                            .and_then(|p| p.invoked_with().as_ref())
                        {
                            if let Some(origin) = &command.originating_plugin {
                                for ((plugin, client, generation), filter) in
                                    &self.pane_output_subscriptions
                                {
                                    if filter.iter().all(|(k, v)| origin.context.get(k) == Some(v))
                                    {
                                        let _ = self.bus.senders.send_to_plugin(
                                            PluginInstruction::for_plugin_event(
                                                *plugin,
                                                *client,
                                                *generation,
                                                Event::PaneOutput {
                                                    pane_id: pid.get(),
                                                    data: bytes.clone(),
                                                    context: origin.context.clone(),
                                                },
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                        tab.handle_pty_bytes(pid, bytes)
                            .context("failed to process pty bytes")?;
                    }
                    break;
                }
            }
        }
        if let Some(vte_bytes) = vte_bytes {
            pending
                .waiting_for_pane
                .entry(PaneId::Terminal(pid))
                .or_default()
                .push(ScreenInstruction::PtyBytes(pid, vte_bytes));
        }
        Ok(())
    }
}

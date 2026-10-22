use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TerminalId;

use super::{PreparedTabWarp, Screen, ScreenInstruction, WarpAddress};
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty::{
    held_command_on_exit, terminal_exit_instruction, AdoptedPane, AdoptedTabPanes,
    AdoptedTabTerminal, ClientTabIndexOrPaneId, PtyInstruction,
};
use crate::pty_host::client::ReleasedPane;
use crate::session_layout_metadata::PaneLayoutMetadata;
use crate::tab::Pane;
use crate::ClientId;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use yinz_bus::warp::ReleasedPaneForWarp;
use yinz_session::warped_tab::layout_for_warped_tab;
use yinz_types::input::command::RunCommand;
use yinz_types::input::layout::Run;
use yinz_types::session_manifest::PaneLayoutManifest;
use {
    yinz_types::data::NewPanePlacement, yinz_types::data::RemotePaneHandoff,
    yinz_types::data::WarpPlacement, yinz_types::data::WarpedPane, yinz_types::data::WarpedTab,
};

/// What Screen keeps back from a tab warp's budget for its own work.
///
/// Once the pty thread reports the release, this thread still has to serialize
/// every pane in the tab — up to 32 MiB of scrollback across the lot, see
/// [`Screen::warp_budget_per_pane`] — build the payload and close the tab, all
/// before the caller's `recv_timeout` fires. Handing the pty thread the caller's
/// whole deadline would mean a release that finished exactly on time still
/// answered too late, and a tab released into a channel nobody is reading is the
/// one outcome this feature must never produce.
///
/// Generous rather than measured: the cost of reserving too much is a warp
/// refused a little early, and the cost of reserving too little is a tab full of
/// orphaned processes.
pub(super) const TAB_WARP_SCREEN_RESERVE: std::time::Duration =
    std::time::Duration::from_millis(1500);

/// The single-pane version of [`TAB_WARP_SCREEN_RESERVE`], and smaller for the
/// obvious reason: one pane's scrollback is capped at
/// [`Screen::WARP_CONTENTS_BUDGET`] rather than a whole tab's worth.
pub(super) const PANE_WARP_SCREEN_RESERVE: std::time::Duration =
    std::time::Duration::from_millis(500);

impl Screen {
    pub(super) fn restore_held_pane(
        &mut self,
        id: PaneId,
        held: &yinz_types::data::HeldTerminal,
    ) -> bool {
        for tab in self.tabs.values_mut() {
            if let Some(pane) = tab.get_pane_with_id_mut(id) {
                pane.hold(held.exit_status, held.first_run, held.command.clone());
                return true;
            }
        }
        false
    }

    // Screen owns held state. Freeze restart input until the release reply is processed;
    // a restart already accepted by Screen is no longer held and takes the supervisor path.
    fn prepare_held_warp(
        &mut self,
        ids: &[TerminalId],
        supported: bool,
    ) -> Result<HashMap<TerminalId, yinz_types::data::HeldTerminal>, String> {
        let mut held = HashMap::new();
        for id in ids {
            if let Some(pane) = self
                .tabs
                .values()
                .find_map(|tab| tab.get_pane_with_id(PaneId::Terminal(*id)))
                && let Some(state) = pane.held_for_warp()
            {
                if !supported {
                    return Err(format!("terminal_{id} ({:?}, held): destination does not support held terminal transfers", pane.current_title()));
                }
                held.insert(*id, state);
            }
        }
        for id in held.keys() {
            for tab in self.tabs.values_mut() {
                if let Some(pane) = tab.get_pane_with_id_mut(PaneId::Terminal(*id)) {
                    pane.set_warp_pending(true);
                }
            }
        }
        Ok(held)
    }

    fn warp_failure(&self, id: TerminalId, reason: String) -> String {
        let pane = self
            .tabs
            .values()
            .find_map(|tab| tab.get_pane_with_id(PaneId::Terminal(id)));
        match pane {
            Some(pane) => format!(
                "terminal_{id} ({:?}, {}): {reason}",
                pane.current_title(),
                if pane.is_held() { "held" } else { "live" }
            ),
            None => format!("terminal_{id}: {reason}"),
        }
    }

    fn validate_held_release(&self, id: TerminalId, released: &ReleasedPane) -> Result<(), String> {
        if let ReleasedPane::Held(held) = released {
            let current = self
                .tabs
                .values()
                .find_map(|tab| tab.get_pane_with_id(PaneId::Terminal(id)))
                .and_then(|pane| pane.held_for_warp());
            if current.as_ref() != Some(held) {
                return Err(
                    self.warp_failure(id, "held state changed during transfer; retry".into())
                );
            }
        }
        Ok(())
    }

    // Run after transfer handling: only surviving held panes can resume here. Live panes
    // keep their queued replacements until normal exit handling fires their hooks.
    fn finish_held_warp(&mut self, ids: impl IntoIterator<Item = TerminalId>) {
        for id in ids {
            let pane_id = PaneId::Terminal(id);
            let pending = self.tab_containing_pane_mut(pane_id).and_then(|tab| {
                if let Some(pane) = tab.get_pane_with_id_mut(pane_id) {
                    pane.set_warp_pending(false);
                    if pane.is_held() {
                        return tab.take_pending_respawn(pane_id);
                    }
                }
                None
            });
            if let Some((command, keep_scrollback)) = pending {
                self.run_command_in_pane_with_id(id, command, keep_scrollback);
            }
        }
    }

    /// The pty thread has released a tab's panes for a warp (a whole tab, or a companion group
    /// that was asked for as one): hand them to the caller still waiting, or strand them
    /// visibly if nobody is.
    pub(super) fn tab_released_for_warp(
        &mut self,
        tab_index: TabId,
        released: Vec<(TerminalId, Result<ReleasedPaneForWarp, String>)>,
    ) {
        let terminal_ids: Vec<_> = released.iter().map(|(id, _)| *id).collect();
        // Asked before the tab is taken apart rather than after.
        // `take_released_tab_for_warp` is what closes the tab, so a
        // caller found to be gone on the far side of it has already
        // cost this session the whole tab, with every process in it
        // running unwatched and a log line as the only record. A caller
        // whose deadline has passed — less the reserve this thread
        // needs, which is what the stored instant already is — is a
        // caller that has stopped waiting, so the tab stays where it
        // is and its panes are held and told why — visible, and
        // recoverable — below.
        if let Some((parked, prepared, deadline)) = self.pending_pane_groups.remove(&tab_index) {
            if Instant::now() < deadline {
                let result = self
                    .take_released_tab_for_warp(tab_index, released, prepared)
                    .map(|group| {
                        Box::new(yinz_types::data::WarpedPane {
                            held: None,
                            group: Some(group),
                            ..Default::default()
                        })
                    });
                self.answer_parked_pane_warp(parked, result);
            } else {
                for (terminal_id, released) in released {
                    if let Ok(released) = released {
                        if matches!(released.pane, ReleasedPane::Held(_)) {
                            continue;
                        }
                        self.bus
                            .senders
                            .send_to_pty(PtyInstruction::StrandPaneAfterWarp {
                                terminal_id,
                                run_command: released.run_command,
                                why: "pane group transfer timed out".into(),
                                recovery: Some(released.pane.recovery_command()),
                            })
                            .non_fatal();
                    }
                }
            }
            self.finish_held_warp(terminal_ids);
            return;
        }
        let still_waiting = match self.pending_tab_warps.remove(&tab_index) {
            Some((_, _, deadline)) if Instant::now() >= deadline => None,
            other => other,
        };
        if let Some((response_channel, prepared, _)) = still_waiting {
            let result = self.take_released_tab_for_warp(tab_index, released, prepared);
            if let Err(crossbeam::channel::SendError(Ok(payload))) = response_channel.send(result) {
                // The caller timed out between asking and being
                // answered. The tab is out of this session by now and
                // every process in it is running unwatched.
                log::error!(
                    "tab {tab_index} was released for a warp nobody was waiting on; its \
                 panes are still running and can be recovered with: {}",
                    payload
                        .tiled_panes
                        .iter()
                        .chain(payload.floating_panes.iter())
                        .map(crate::warp::recovery_command)
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            }
        } else {
            // Nobody is waiting — the caller gave up, or left too
            // little time to serialize the tab in — which means the
            // supervisors have been released with no one to hand them
            // to. The tab is still on screen and every pane in it is
            // now talking to nothing, so saying where the processes
            // went in the log is not enough — each pane has to say it
            // too, or the user is left with a tab that looks alive and
            // answers nothing.
            log::error!(
                "tab {tab_index} was released for a warp nobody was waiting on any \
             more; the tab was kept, and its panes are still running and can be \
             recovered with `yinz pty-host adopt <id>`"
            );
            for (terminal_id, released) in released {
                if let Ok(released) = released {
                    if matches!(released.pane, ReleasedPane::Held(_)) {
                        continue;
                    }
                    self.bus
                        .senders
                        .send_to_pty(PtyInstruction::StrandPaneAfterWarp {
                            terminal_id,
                            run_command: released.run_command,
                            why: "this tab was moved to another session, but nothing took it"
                                .to_owned(),
                            recovery: Some(released.pane.recovery_command()),
                        })
                        .non_fatal();
                }
            }
        }
        self.finish_held_warp(terminal_ids);
    }

    /// Starts a tab warp: refuses it outright if it cannot proceed, otherwise asks the pty thread
    /// to release every pane in the tab and parks the caller's channel until
    /// `TabReleasedForWarp` answers it.
    pub(super) fn warp_tab(
        &mut self,
        supports_companions: bool,
        supports_held: bool,
        tab_id: Option<TabId>,
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<Result<Box<WarpedTab>, String>>,
        deadline: Instant,
    ) -> Result<()> {
        // Every check that can fail happens before the pty thread is
        // asked for anything, so a tab whose warp is going to be refused
        // stays exactly where it was.
        match self.check_tab_can_be_warped(tab_id, client_id) {
            Ok((tab_index, terminal_ids, prepared)) => {
                if prepared.relationships.is_some() && !supports_companions {
                    let _ = response_channel.send(Err(
                        "destination does not support companion pane transfers".into(),
                    ));
                    return Ok(());
                }
                if self.pending_pane_groups.contains_key(&tab_index)
                    || self.pending_tab_warps.contains_key(&tab_index)
                {
                    let _ = response_channel
                        .send(Err("this tab already has a warp in progress".into()));
                    return Ok(());
                }
                // The caller's deadline less what this thread will
                // need afterwards: `take_released_tab_for_warp`
                // serializes every pane's scrollback, and a release
                // that used the whole budget would leave no time to do
                // it in. `checked_sub` for the reason given in
                // `WarpPane` above. One number for both the pty thread
                // and the check in `TabReleasedForWarp`: a release
                // that lands inside the reserve is one this thread can
                // no longer serialize in time, however punctual the
                // pty thread was.
                let deadline = deadline
                    .checked_sub(TAB_WARP_SCREEN_RESERVE)
                    .unwrap_or_else(Instant::now);
                let held = match self.prepare_held_warp(&terminal_ids, supports_held) {
                    Ok(held) => held,
                    Err(reason) => {
                        let _ = response_channel.send(Err(reason));
                        return Ok(());
                    },
                };
                self.pending_tab_warps
                    .insert(tab_index, (response_channel, prepared, deadline));
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::ReleaseTerminalsForWarp {
                        held,
                        tab_index,
                        terminal_ids,
                        deadline,
                    })?;
            },
            Err(e) => {
                let _ = response_channel.send(Err(e));
            },
        }
        Ok(())
    }

    /// The pty thread has released a pane for a warp: hand it to the caller still waiting for
    /// it, or strand it visibly if nobody is.
    pub(super) fn pane_released_for_warp(
        &mut self,
        terminal_id: TerminalId,
        released: Result<ReleasedPaneForWarp, String>,
    ) {
        // Asked before the pane is taken apart, exactly as
        // `TabReleasedForWarp` asks below and for the same reason:
        // `take_released_pane_for_warp` is what closes the pane, so a
        // caller found to be gone on the far side of it has already
        // cost the user the pane, with its process running unwatched
        // and a log line as the only record. A caller past its
        // deadline has stopped waiting; the pane stays where it is,
        // held and told why.
        let still_waiting = match self.pending_warps.remove(&terminal_id) {
            Some((_, deadline)) if Instant::now() >= deadline => None,
            other => other,
        };
        if let Some((parked, _)) = still_waiting {
            let result = self.take_released_pane_for_warp(terminal_id, released);
            self.answer_parked_pane_warp(parked, result);
        } else {
            // Nobody is waiting, which means the supervisor has been
            // released with no one to hand it to. The pane is still
            // on screen and now talking to nothing, so it has to say
            // so itself — a log line alone leaves the user with a pane
            // that looks alive and answers nothing. A release that
            // *failed* was already stranded by the pty thread, with
            // the failure as its reason.
            log::error!(
                "pane {terminal_id} was released for a warp nobody was waiting on; it is \
             still running and can be recovered with `yinz pty-host adopt <id>`"
            );
            if let Ok(released) = released
                && !matches!(released.pane, ReleasedPane::Held(_))
            {
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::StrandPaneAfterWarp {
                        terminal_id,
                        run_command: released.run_command,
                        why: "this pane was moved to another session, but nothing took it"
                            .to_owned(),
                        recovery: Some(released.pane.recovery_command()),
                    })
                    .non_fatal();
            }
        }
        self.finish_held_warp([terminal_id]);
    }

    /// Hand a released pane to the caller that parked the warp, along with its hold.
    ///
    /// The caller can time out between asking and being answered. The pane is out of
    /// its tab by then, so it is taken back here rather than left running unwatched,
    /// and the hold goes with it so the session stays open until it is placed.
    fn answer_parked_pane_warp(
        &mut self,
        parked: super::ParkedPaneWarp,
        result: Result<Box<WarpedPane>, String>,
    ) {
        let super::ParkedPaneWarp {
            response_channel,
            hold,
            placement,
            client_id,
        } = parked;
        let Some((payload, hold)) = response_channel.answer(result.map(|payload| (payload, hold)))
        else {
            return;
        };
        log::error!(
            "a pane was released for a warp nobody was waiting on; taking it back. If that \
             fails, recover it with `{}`",
            crate::warp::recovery_command(&payload)
        );
        let (response_channel, _) = crossbeam::channel::unbounded();
        let (placed, _) = crossbeam::channel::unbounded();
        self.bus
            .senders
            .send_to_pty(PtyInstruction::AdoptTerminal {
                payload,
                placement,
                client_id,
                response_channel,
                hold: Some(hold),
                placed,
            })
            .context("failed to take back an unclaimed pane")
            .non_fatal();
    }

    /// Park a pane warp's caller, holding the session open until the warp resolves.
    fn park_pane_warp(
        &mut self,
        response_channel: yinz_bus::warp::PaneWarpReply,
        placement: WarpPlacement,
        client_id: ClientId,
    ) -> super::ParkedPaneWarp {
        self.in_flight_pane_warps += 1;
        super::ParkedPaneWarp {
            response_channel,
            hold: yinz_bus::warp::PaneWarpHold::new(self.bus.senders.clone()),
            placement,
            client_id,
        }
    }

    /// Starts a pane warp: refuses it outright if it cannot proceed, otherwise asks the pty
    /// thread to release the pane (or, for a companion group, every pane in it) and parks the
    /// caller's channel until `PaneReleasedForWarp` / `TabReleasedForWarp` answers it.
    pub(super) fn warp_pane(
        &mut self,
        supports_companions: bool,
        supports_held: bool,
        pane_id: Option<PaneId>,
        to_session: SessionName,
        placement: WarpPlacement,
        client_id: ClientId,
        response_channel: yinz_bus::warp::PaneWarpReply,
        deadline: Instant,
    ) -> Result<()> {
        // Every check that can fail happens before the pty thread is asked
        // for anything: a warp that is going to be refused must leave the
        // pane exactly where it was.
        match self.prepare_pane_group(pane_id, client_id) {
            Ok(Some((tab_index, terminal_ids, prepared))) => {
                if !supports_companions {
                    let _ = response_channel.answer(Err(
                        "destination does not support companion pane transfers".into(),
                    ));
                    return Ok(());
                }
                if self.pending_pane_groups.contains_key(&tab_index)
                    || self.pending_tab_warps.contains_key(&tab_index)
                {
                    let _ = response_channel
                        .answer(Err("this tab already has a warp in progress".into()));
                    return Ok(());
                }
                let deadline = deadline
                    .checked_sub(TAB_WARP_SCREEN_RESERVE)
                    .unwrap_or_else(Instant::now);
                let held = match self.prepare_held_warp(&terminal_ids, supports_held) {
                    Ok(held) => held,
                    Err(reason) => {
                        let _ = response_channel.answer(Err(reason));
                        return Ok(());
                    },
                };
                let parked = self.park_pane_warp(response_channel, placement, client_id);
                self.pending_pane_groups
                    .insert(tab_index, (parked, prepared, deadline));
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::ReleaseTerminalsForWarp {
                        held,
                        tab_index,
                        terminal_ids,
                        deadline,
                    })?;
                return Ok(());
            },
            Err(e) => {
                let _ = response_channel.answer(Err(e));
                return Ok(());
            },
            Ok(None) => {},
        }
        match self.check_pane_can_be_warped(pane_id, client_id) {
            Ok(terminal_id) => {
                // Minus what this thread needs to serialize the pane
                // once the release lands, so the answer still beats
                // the caller's clock. `checked_sub` because `Instant -
                // Duration` panics rather than saturating, and a panic
                // here would take the screen thread with it; a deadline
                // that has already passed is the honest answer for a
                // caller with nothing left to spend. The same instant
                // is what `PaneReleasedForWarp` checks before it starts
                // that serialization, for the same reason.
                let deadline = deadline
                    .checked_sub(PANE_WARP_SCREEN_RESERVE)
                    .unwrap_or_else(Instant::now);
                let held = match self.prepare_held_warp(&[terminal_id], supports_held) {
                    Ok(held) => held,
                    Err(reason) => {
                        let _ = response_channel.answer(Err(reason));
                        return Ok(());
                    },
                };
                let parked = self.park_pane_warp(response_channel, placement, client_id);
                self.pending_warps.insert(terminal_id, (parked, deadline));
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::ReleaseTerminalForWarp {
                        held: held.get(&terminal_id).cloned(),
                        terminal_id,
                        to_session,
                        placement,
                        client_id,
                        deadline,
                    })?;
            },
            Err(e) => {
                let _ = response_channel.answer(Err(e));
            },
        }
        Ok(())
    }

    fn notify_warp_departure(&mut self, pane_id: PaneId) {
        self.invalidate_agent_lifecycle(pane_id, "warped");
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                yinz_types::data::Event::PaneClosed(pane_id),
            )]))
            .non_fatal();
        self.notify_pane_closed_to_subscribers(pane_id);
    }

    pub(super) fn finish_warp_group(
        &mut self,
        tab_id: TabId,
        slots: &[PaneId],
        relationships: &yinz_types::data::WarpRelationships,
    ) -> Result<()> {
        use crate::tab::{AcceptedPane, ExtractedPanePlacement, SuppressedPane};
        // `_hold` keeps a rollback's source session open until the group is placed.
        let Some((placement, target, client_id, response_channel, _hold)) =
            self.pending_group_placements.remove(&tab_id)
        else {
            return Ok(());
        };
        let result = (|| {
            if placement == WarpPlacement::NewTab {
                return Ok(());
            }
            let Some(target) = target.filter(|id| self.tabs.contains_key(id)) else {
                return Ok(());
            };
            let source = self
                .tabs
                .get_mut(&tab_id)
                .context("missing adopted group tab")?;
            let mut panes = source.extract_warp_panes(slots).into_iter();
            let root = panes.next().context("missing adopted group root")?;
            let placement = if placement == WarpPlacement::Floating {
                let min_height = relationships
                    .stacks
                    .iter()
                    .find(|stack| stack.selected == 0)
                    // Headers, padding, bottom frame, and at least one content row.
                    .map_or(0, |stack| stack.members.len() + 3);
                ExtractedPanePlacement::FloatingDefault { min_height }
            } else {
                ExtractedPanePlacement::Tiled
            };
            let destination = self
                .tabs
                .get_mut(&target)
                .context("missing warp destination")?;
            match destination.accept_extracted_pane(root, placement) {
                AcceptedPane::Accepted => {},
                AcceptedPane::NoRoom(root) => {
                    let source = self
                        .tabs
                        .get_mut(&tab_id)
                        .context("missing group recovery tab")?;
                    source.add_tiled_pane(root, slots[0], true, Some(client_id))?;
                    source.add_suppressed_panes(
                        panes
                            .map(|pane| {
                                (
                                    pane.pid(),
                                    SuppressedPane {
                                        pane,
                                        restored_by: None,
                                    },
                                )
                            })
                            .collect(),
                    );
                    source.restore_warp_relationships(slots, relationships)?;
                    // Retaining the adopted group is success; the source must not roll it back.
                    return Ok(());
                },
                AcceptedPane::LostInAdd => {
                    return Err(anyhow!("the destination lost the adopted root pane"))
                },
            }
            destination.add_suppressed_panes(
                panes
                    .map(|pane| {
                        (
                            pane.pid(),
                            SuppressedPane {
                                pane,
                                restored_by: None,
                            },
                        )
                    })
                    .collect(),
            );
            destination.restore_warp_relationships(slots, relationships)?;
            destination.focus_pane_with_id(slots[0], false, false, client_id)?;
            self.close_tab_by_id(tab_id)?;
            if let Some(position) = self.get_tab_position_by_id(target) {
                self.switch_active_tab(position, None, true, client_id)?;
            }
            Ok(())
        })();
        if let Some(response_channel) = response_channel {
            let _ = response_channel.send(result);
            Ok(())
        } else {
            result
        }
    }

    pub(super) fn prepare_pane_group(
        &self,
        pane_id: Option<PaneId>,
        client_id: ClientId,
    ) -> Result<Option<(TabId, Vec<TerminalId>, PreparedTabWarp)>, String> {
        let pane_id = pane_id
            .or_else(|| self.active_pane_id(client_id))
            .ok_or_else(|| "no pane is focused".to_owned())?;
        let (tab_id, tab) = self
            .tabs
            .iter()
            .find(|(_, tab)| tab.get_pane_with_id(pane_id).is_some())
            .ok_or_else(|| format!("there is no pane {pane_id:?} in this session"))?;
        let ids = tab.warp_pane_ids(Some(pane_id))?;
        if ids.len() == 1 && matches!(pane_id, PaneId::Terminal(_)) {
            return Ok(None);
        }
        let mut slots = vec![pane_id];
        slots.extend(ids.into_iter().filter(|id| *id != pane_id));
        let root = tab
            .get_pane_with_id(pane_id)
            .ok_or_else(|| "warp root vanished".to_owned())?;
        let tiled_layout = yinz_types::input::layout::TiledPaneLayout {
            run: root.invoked_with().clone(),
            name: carried_caption(root),
            borderless: Some(root.borderless()),
            ..Default::default()
        };
        let floating_layout = slots
            .iter()
            .skip(1)
            .map(|id| {
                let pane = tab
                    .get_pane_with_id(*id)
                    .ok_or_else(|| "warp companion vanished".to_owned())?;
                Ok(yinz_types::input::layout::FloatingPaneLayout {
                    run: pane.invoked_with().clone(),
                    name: carried_caption(pane),
                    borderless: Some(pane.borderless()),
                    ..Default::default()
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let relationships = tab.warp_relationships(&slots);
        relationships.validate(slots.len())?;
        let terminals = |panes: &[PaneId]| {
            panes
                .iter()
                .copied()
                .filter(|id| matches!(id, PaneId::Terminal(_)))
                .collect::<Vec<_>>()
        };
        let terminal_ids = slots
            .iter()
            .filter_map(|id| {
                if let PaneId::Terminal(id) = id {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        Ok(Some((
            *tab_id,
            terminal_ids,
            PreparedTabWarp {
                name: None,
                user_named: false,
                tiled_layout,
                floating_layout,
                tiled_panes: terminals(&slots[..1]),
                floating_panes: terminals(&slots[1..]),
                hide_floating_panes: true,
                relationships: Some(relationships),
                slots,
                pane_group: true,
            },
        )))
    }

    /// Roughly 200 columns of styled text over a few thousand lines. Well inside
    /// `MAX_IPC_FRAME_LEN`, and far past what anyone reads back.
    pub(super) const WARP_CONTENTS_BUDGET: usize = 4 * 1024 * 1024;

    /// What one pane of a warped tab may spend on its scrollback.
    ///
    /// A whole tab travels in a single frame, so the per-pane budget cannot also
    /// be the whole budget: sixteen panes at 4 MiB each would be exactly
    /// `MAX_IPC_FRAME_LEN`, and the seventeenth would make the frame
    /// unsendable — losing the tab at the last step, after every supervisor had
    /// already been released. Sharing a fixed allowance out means a tab of any
    /// size fits, and only the scrollback gets shorter.
    pub(super) fn warp_budget_per_pane(panes: usize) -> usize {
        /// Half of `MAX_IPC_FRAME_LEN`, leaving the layout, the titles and
        /// protobuf's own framing somewhere to live.
        const WARP_TAB_BUDGET: usize = 32 * 1024 * 1024;

        (WARP_TAB_BUDGET / panes.max(1)).min(Self::WARP_CONTENTS_BUDGET)
    }

    #[cfg(test)]
    pub(super) fn fit_to_budget(contents: String, budget: usize) -> String {
        yinz_terminal::agent_theme::fit_legacy_contents(contents, budget)
    }

    /// Check the ordinary terminal path after companion-group preparation.
    /// Hidden IDs are rejected here too, before asking the pty thread to release.
    pub(super) fn check_pane_can_be_warped(
        &mut self,
        pane_id: Option<PaneId>,
        client_id: ClientId,
    ) -> Result<TerminalId, String> {
        let pane_id = match pane_id {
            Some(pane_id) => pane_id,
            None => self
                .active_pane_id(client_id)
                .ok_or_else(|| "no pane is focused".to_owned())?,
        };
        let terminal_id = match pane_id {
            PaneId::Terminal(terminal_id) => terminal_id,
            PaneId::Plugin(_) => {
                return Err(
                    "this is a plugin pane; it has no process, so there is nothing to move"
                        .to_owned(),
                );
            },
        };

        let tab = self
            .tabs
            .values()
            .find(|tab| tab.get_pane_with_id(pane_id).is_some())
            .ok_or_else(|| format!("there is no pane {terminal_id} in this session"))?;
        tab.warp_pane_ids(Some(pane_id))?;
        Ok(terminal_id)
    }

    /// How a released pane says where its process can be picked up.
    ///
    /// The one thing a `WarpedPane` cannot get from the tab. A local supervisor
    /// that never bound a socket has no address to hand out — but the pty
    /// thread's pre-flight check has already refused that pane before releasing
    /// anything, so reaching the refusal below means something changed between
    /// the check and the release. It is kept as the backstop for that, not as
    /// the place the case is expected to be caught.
    ///
    /// The check has no remote counterpart — a broker is reached by ssh and a
    /// token, and a pane the broker confirmed it released is reachable by
    /// definition.
    fn warp_address(released: ReleasedPane) -> Result<WarpAddress, String> {
        match released {
            ReleasedPane::Held(held) => Ok(WarpAddress {
                held: Some(held),
                ..Default::default()
            }),
            ReleasedPane::Local(local) => {
                let Some(control_socket) = local.control_socket else {
                    // No address to hand out. The pty thread refuses this before
                    // releasing anything, so a pane that gets here has already
                    // been cut loose and has been told so in its own scrollback;
                    // there is nothing left for this branch to do but say why.
                    return Err(format!(
                        "the supervisor for this pane ({}) never bound a socket, so no other \
                         session can reach it — this usually means the runtime directory path \
                         is too long",
                        local.host_id
                    ));
                };
                Ok(WarpAddress {
                    held: None,
                    host_id: local.host_id,
                    control_socket,
                    remote: None,
                })
            },
            ReleasedPane::Remote(remote) => Ok(WarpAddress {
                held: None,
                // Both empty on purpose rather than filled with something
                // plausible: they name a local supervisor socket, and reading
                // them for a remote pane is a mistake `remote` being `Some` is
                // there to catch.
                host_id: SupervisorId::default(),
                control_socket: PathBuf::new(),
                remote: Some(RemotePaneHandoff {
                    host: remote.host,
                    pane_token: remote.pane_token,
                    received: remote.received,
                    cmd: remote.cmd,
                }),
            }),
        }
    }

    /// Serialize an ordinary pane after its reader has stopped. Companion groups
    /// use the batch path below, including during rollback.
    pub(super) fn take_released_pane_for_warp(
        &mut self,
        terminal_id: TerminalId,
        released: Result<ReleasedPaneForWarp, String>,
    ) -> Result<Box<WarpedPane>, String> {
        let released = released.map_err(|reason| self.warp_failure(terminal_id, reason))?;
        self.validate_held_release(terminal_id, &released.pane)?;
        let address = Self::warp_address(released.pane)?;
        let pane_id = PaneId::Terminal(terminal_id);
        let agent_input = self.warped_agent_input(pane_id);
        let scrollback_lines = self.scrollback_lines_to_serialize;
        let tab = self
            .tabs
            .values_mut()
            .find(|tab| tab.get_pane_with_id(pane_id).is_some())
            .ok_or_else(|| {
                format!("pane {terminal_id} vanished while its supervisor was being released")
            })?;
        let pane = tab
            .get_pane_with_id(pane_id)
            .ok_or_else(|| "warp pane vanished".to_owned())?;
        let (contents, agent_theme) = pane
            .serialize_handoff(scrollback_lines, Self::WARP_CONTENTS_BUDGET)
            .map(|(c, m)| (Some(c), m))
            .unwrap_or_default();
        let payload = Box::new(WarpedPane {
            agent_theme,
            held: address.held,
            group: None,
            host_id: address.host_id,
            control_socket: address.control_socket,
            remote: address.remote,
            title: carried_caption(pane),
            caption: default_caption(pane),
            contents,
            borderless: pane.borderless(),
            bracketed_paste: pane.supports_bracketed_paste(),
            agent_input,
            agent: pane.agent_info().cloned(),
        });
        let removed = tab.extract_warp_panes(&[pane_id]);
        for pane in removed {
            if payload.held.is_some() {
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::ClosePane(pane.pid(), None))
                    .non_fatal();
            }
            self.notify_warp_departure(pane.pid());
        }
        self.clear_nested_guest(pane_id);
        Ok(payload)
    }

    /// Work out how a tab would be moved, so that everything which can fail has
    /// failed before anything is released.
    ///
    /// The arrangement is built *here*, not after the release, and that ordering
    /// is the point: a tab whose supervisors had already been let go by the time
    /// something failed would be left on screen with nothing attached to it. Not
    /// every tiling is a layout — see `tiled_layout_from_panegeoms_lossy` — but
    /// that is approximated rather than refused, so what is left to fail here is
    /// the checks above. Only the two things that genuinely cannot be known yet
    /// are left for later: which socket each supervisor ended up on, and what was
    /// on each pane's screen once its reader had stopped.
    ///
    /// `tab_index` is a **stable tab id**, not a display position — `self.tabs` is
    /// keyed by id, and the focused-tab default below reads `active_tab_ids`. The
    /// name is historical; a caller that hands over a position addresses whichever
    /// tab owns that number as an id, which diverges the moment a tab closes.
    pub(super) fn check_tab_can_be_warped(
        &mut self,
        tab_index: Option<TabId>,
        client_id: ClientId,
    ) -> Result<(TabId, Vec<TerminalId>, PreparedTabWarp), String> {
        let tab_index = match tab_index {
            Some(tab_index) => tab_index,
            None => self
                .active_tab_ids
                .get(&client_id)
                .copied()
                .ok_or_else(|| "no tab is focused".to_owned())?,
        };
        let tab = self
            .tabs
            .get(&tab_index)
            .ok_or_else(|| format!("there is no tab {tab_index} in this session"))?;

        let transfer_ids = tab.warp_pane_ids(None)?;
        if transfer_ids.is_empty() {
            return Err("this tab has no panes to move".into());
        }

        let name = Some(tab.name.clone()).filter(|name| !name.is_empty());
        let user_named = tab.is_user_named();
        let viewport = tab.size;
        // Contents are left out: the reader tasks are still running, so anything
        // serialized now would be missing whatever arrives between here and the
        // release. They are read again in `take_released_tab_for_warp`, by which
        // time the screen has stopped changing.
        let (tiled, floating, hide_floating_panes) =
            self.tab_panes_for_layout(tab_index, tab, false, true);
        let manifests =
            |panes: &[(PaneId, PaneLayoutMetadata)]| -> Vec<(PaneId, PaneLayoutManifest)> {
                panes
                    .iter()
                    .map(|(pane_id, pane)| {
                        let mut manifest: PaneLayoutManifest = pane.clone().into();
                        // More than the manifest's own `custom_title`, which is
                        // all a *layout* should pin down: a warp is moving the
                        // pane the user is looking at, so a title its program set
                        // arrives too. See `carried_caption` for what stays behind.
                        manifest.title = tab
                            .get_pane_with_id(*pane_id)
                            .and_then(carried_caption)
                            .or(manifest.title);
                        (*pane_id, manifest)
                    })
                    .collect()
            };
        let tiled = manifests(&tiled);
        let floating = manifests(&floating);

        let mut arrangement = layout_for_warped_tab(
            &tiled
                .iter()
                .map(|(_, pane)| pane.clone())
                .collect::<Vec<_>>(),
            &floating
                .iter()
                .map(|(_, pane)| pane.clone())
                .collect::<Vec<_>>(),
            viewport,
        )?;
        arrangement.tiled_layout.tab_group = self.tab_group_info(tab_index);

        let mut slots: Vec<PaneId> = arrangement
            .all_tiled_order
            .iter()
            .map(|i| tiled[*i].0)
            .chain(floating.iter().map(|(id, _)| *id))
            .collect();
        let tiled_panes = arrangement
            .tiled_order
            .iter()
            .map(|i| tiled[*i].0)
            .filter(|id| matches!(id, PaneId::Terminal(_)))
            .collect::<Vec<_>>();
        let mut floating_panes = arrangement
            .floating_order
            .iter()
            .map(|i| floating[*i].0)
            .filter(|id| matches!(id, PaneId::Terminal(_)))
            .collect::<Vec<_>>();
        for id in transfer_ids {
            if slots.contains(&id) {
                continue;
            }
            let pane = tab
                .get_pane_with_id(id)
                .ok_or_else(|| format!("missing companion {id:?}"))?;
            arrangement
                .floating_layout
                .push(yinz_types::input::layout::FloatingPaneLayout {
                    name: carried_caption(pane),
                    run: pane.invoked_with().clone(),
                    borderless: Some(pane.borderless()),
                    ..Default::default()
                });
            slots.push(id);
            if matches!(id, PaneId::Terminal(_)) {
                floating_panes.push(id);
            }
        }
        let relationships = tab.warp_relationships(&slots);
        relationships.validate(slots.len())?;
        let relationships = (!relationships.hidden.is_empty() || !relationships.stacks.is_empty())
            .then_some(relationships);

        // Released from the slots rather than from the tab's pane ids, so that the
        // two sets cannot drift apart. `take_released_tab_for_warp` builds the
        // payload by walking the slots, so a terminal released without one would
        // be handed over to nobody — and by then it is too late to say so.
        //
        // Plugin panes have no slot here on purpose. They have no process to hand
        // over, but they do have a `Run::Plugin` in the layout, so the receiving
        // session builds the same tab by starting them afresh.
        let terminal_ids: Vec<TerminalId> = tiled_panes
            .iter()
            .chain(floating_panes.iter())
            .filter_map(|pane_id| match pane_id {
                PaneId::Terminal(terminal_id) => Some(*terminal_id),
                PaneId::Plugin(_) => None,
            })
            .collect();

        Ok((
            tab_index,
            terminal_ids,
            PreparedTabWarp {
                relationships,
                slots,
                pane_group: false,
                name,
                user_named,
                tiled_layout: arrangement.tiled_layout,
                floating_layout: arrangement.floating_layout,
                tiled_panes,
                floating_panes,
                hide_floating_panes,
            },
        ))
    }

    /// Fill in what only the release could tell us, and take the tab out of this
    /// session.
    ///
    /// Everything fallible about the *arrangement* was settled in
    /// `check_tab_can_be_warped`, before any supervisor was let go. What is left
    /// here genuinely could not have been done earlier: which socket each
    /// supervisor ended up on, and what each pane's screen finally said — which
    /// is only reliable now, because the release means the reader tasks are gone
    /// and everything they emitted is already queued ahead of this.
    ///
    /// Closing the tab is the other careful part. `close_tab_by_id` ends by
    /// telling the pty thread to close whatever panes are left, which would
    /// SIGHUP every child; taking the panes out first leaves it nothing to kill.
    pub(super) fn take_released_tab_for_warp(
        &mut self,
        tab_index: TabId,
        released: Vec<(TerminalId, Result<ReleasedPaneForWarp, String>)>,
        prepared: PreparedTabWarp,
    ) -> Result<Box<WarpedTab>, String> {
        // PTY release failures already strand every detached peer. Check those first
        // so a concurrent held-state change does not send duplicate recovery notices.
        for (terminal_id, result) in &released {
            if let Err(reason) = result {
                return Err(self.warp_failure(*terminal_id, reason.clone()));
            }
        }
        for (terminal_id, result) in &released {
            if let Ok(pane) = result
                && let Err(reason) = self.validate_held_release(*terminal_id, &pane.pane)
            {
                // Keep every release address until validation finishes: live peers
                // before and after this held pane have already been detached.
                for (terminal_id, result) in &released {
                    if let Ok(released) = result
                        && !matches!(released.pane, ReleasedPane::Held(_))
                    {
                        self.bus
                            .senders
                            .send_to_pty(PtyInstruction::StrandPaneAfterWarp {
                                terminal_id: *terminal_id,
                                run_command: released.run_command.clone(),
                                why: reason.clone(),
                                recovery: Some(released.pane.recovery_command()),
                            })
                            .non_fatal();
                    }
                }
                return Err(reason);
            }
        }
        let mut addresses: HashMap<TerminalId, WarpAddress> = HashMap::new();
        for (terminal_id, released) in released {
            // The refusal `warp_address` can produce is worded for one pane, and
            // a tab is refused whole for it. Reaching it at all means the pty
            // thread's pre-flight check passed and the release then failed, in
            // which case that thread has already told every pane in the tab what
            // happened to it — this is the reason travelling back to the caller,
            // not the pane's only account of itself.
            let released = released.map_err(|reason| self.warp_failure(terminal_id, reason))?;
            addresses.insert(terminal_id, Self::warp_address(released.pane)?);
        }

        // The budget is shared out only now, because only now is it known how
        // many panes are sharing it.
        let budget =
            Self::warp_budget_per_pane(prepared.tiled_panes.len() + prepared.floating_panes.len());
        let scrollback_lines = self.scrollback_lines_to_serialize;
        let tab = self
            .tabs
            .get(&tab_index)
            .ok_or_else(|| format!("tab {tab_index} vanished while it was being released"))?;
        let warped_pane = |pane_id: &PaneId| {
            // A plugin pane has no process and so no address; it rides along as
            // its `Run::Plugin` layout node and starts afresh in the same slot.
            let address = match pane_id {
                PaneId::Terminal(terminal_id) => addresses.get(terminal_id).cloned(),
                PaneId::Plugin(_) => None,
            }
            .unwrap_or_default();
            let (contents, agent_theme) = tab
                .get_pane_with_id(*pane_id)
                .and_then(|p| p.serialize_handoff(scrollback_lines, budget))
                .map(|(c, m)| (Some(c), m))
                .unwrap_or_default();
            WarpedPane {
                agent_theme,
                held: address.held,
                group: None,
                host_id: address.host_id,
                control_socket: address.control_socket,
                remote: address.remote,
                // A tab's titles and borders ride in its layout, which is what
                // the receiving session reads when it builds the panes. Repeating
                // them here would be a second answer to the same question. A
                // default caption cannot ride there: a layout's name is explicit.
                title: None,
                caption: tab.get_pane_with_id(*pane_id).and_then(default_caption),
                bracketed_paste: tab
                    .get_pane_with_id(*pane_id)
                    .is_some_and(|pane| pane.supports_bracketed_paste()),
                agent_input: self.warped_agent_input(*pane_id),
                borderless: false,
                // Not gated on `serialize_pane_viewport`: that option governs
                // automatic session serialization, and a warp is something the
                // user asked for by name.
                contents,
                agent: tab
                    .get_pane_with_id(*pane_id)
                    .and_then(|pane| pane.agent_info().cloned()),
            }
        };
        let tiled_panes: Vec<WarpedPane> = prepared.tiled_panes.iter().map(warped_pane).collect();
        let floating_panes: Vec<WarpedPane> =
            prepared.floating_panes.iter().map(warped_pane).collect();

        let tab = self
            .tabs
            .get_mut(&tab_index)
            .ok_or_else(|| "warp tab disappeared".to_owned())?;
        let removed = tab.extract_warp_panes(&prepared.slots);
        for pane in removed {
            let id = pane.pid();
            if matches!(id, PaneId::Terminal(t) if addresses.get(&t).is_some_and(|a| a.held.is_some()))
            {
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::ClosePane(id, None))
                    .non_fatal();
            }
            self.notify_warp_departure(id);
            if let PaneId::Plugin(plugin_id) = id {
                self.drop_quit_notice(plugin_id);
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::ClosePane(id, None))
                    .non_fatal();
            }
            self.clear_nested_guest(id);
        }
        if !prepared.pane_group {
            self.empty_and_close_tab(tab_index);
        }

        Ok(Box::new(WarpedTab {
            source_instance_id: (!prepared.pane_group).then(|| self.instance_id.to_string()),
            relationships: prepared.relationships,
            name: prepared.name,
            user_named: prepared.user_named,
            tiled_layout: prepared.tiled_layout,
            floating_layout: prepared.floating_layout,
            tiled_panes,
            floating_panes,
            hide_floating_panes: prepared.hide_floating_panes,
        }))
    }

    /// Take a warped tab's own panes out of it and then close it.
    ///
    /// `close_tab_by_id` ends by telling the pty thread to close whatever panes
    /// are left, which for a warped tab would kill the processes that are the
    /// whole point of moving it. Emptying the tab first leaves that instruction
    /// with nothing to act on.
    ///
    /// The tab's own panes, which is not the same as `get_all_pane_ids`: that also
    /// reports the suppressed *plugin* panes and the overlays, and closing one
    /// would take it away from the whole session rather than from this tab.
    /// `close_tab_by_id` already knows to re-home both. The suppressed panes that do have to go are the
    /// hidden members of the tab's stack lists — they are travelling, and being
    /// re-homed would leave them in another tab with their supervisors handed away.
    pub(super) fn empty_and_close_tab(&mut self, tab_index: TabId) {
        let mut emptied = vec![];
        if let Some(tab) = self.tabs.get_mut(&tab_index) {
            // Hidden stack-list members first: closing a list's *visible* member
            // promotes a hidden one into the grid, so sweeping the tiled panes
            // while members were still parked would keep handing the sweep panes
            // it had already passed. `close_pane` routes each one through
            // `close_stack_list_member`, which does the list's bookkeeping and,
            // like the rest of this function, tells the pty thread nothing.
            let hidden_members: Vec<PaneId> = tab
                .suppressed_stack_list_members()
                .map(|(pane_id, _)| *pane_id)
                .collect();
            for pane_id in hidden_members {
                tab.close_pane(pane_id, false, None);
                emptied.push(pane_id);
            }
            // Only what was released, which is what the tiled and floating slots were built from.
            // An overlay has no slot and so was never handed over (see
            // `Tab::tiled_and_floating_pane_ids`); taking it out here would remove it from the tab
            // before `close_tab_by_id` could see it, and that is the one thing that knows what to
            // do with one. Left where it is, it is re-homed into a surviving tab
            // (`Screen::move_overlay_panes_from_closed_tab`) - which is right for a warp too: an
            // overlay is a local plugin instance with no process to hand over, so it stays in this
            // session rather than travelling with the tab.
            for pane_id in tab.tiled_and_floating_pane_ids() {
                tab.close_pane(pane_id, false, None);
                emptied.push(pane_id);
            }
        }
        for pane_id in emptied {
            if let PaneId::Plugin(plugin_id) = pane_id {
                self.drop_quit_notice(plugin_id);
            }
            self.clear_nested_guest(pane_id);
        }
        self.close_tab_by_id(tab_index).non_fatal();
    }

    /// Give an adopted supervisor a pane in this session.
    ///
    /// Deliberately expressed as the ordinary pane-creation instructions rather
    /// than by building a `TerminalPane` here: a pane object is full of
    /// per-session `Rc`s and could never have been carried across anyway, and
    /// `NewPane` already knows how to place one.
    pub(super) fn place_adopted_pane(
        &mut self,
        terminal_id: TerminalId,
        mut payload: WarpedPane,
        placement: WarpPlacement,
        client_id: ClientId,
        cmd: RunCommand,
        buffered: Vec<u8>,
        child_exit_status: Option<Option<i32>>,
        finish: &mut Option<yinz_bus::warp::AdoptedPaneFinish>,
    ) -> Result<()> {
        let err_context = || format!("failed to place adopted pane {terminal_id}");
        let request_redraw = payload.held.is_none() && child_exit_status.is_none();
        let pane_id = PaneId::Terminal(terminal_id);
        let invoked_with = Some(Run::Command(cmd.clone()));
        // A pane whose child is already gone should arrive held, showing its exit
        // status, rather than pretending to run — but only if it would have been
        // held in the session it came from. Otherwise it is closed below, exactly
        // as the supervisor's replayed exit would close it through `quit_cb`.
        // An exited held pane is restored below. Creating it as first-run would install
        // a startup banner that overwrites its transferred scrollback on resize.
        let hold_for_command = match &payload.held {
            Some(held) => held.first_run.then(|| cmd.clone()),
            None => held_command_on_exit(&cmd, child_exit_status),
        };
        let focus_client = match placement {
            WarpPlacement::FocusedTab => self.connected_client_to_act_for(client_id),
            _ => None,
        };

        let (placement, tab_target) = match placement {
            WarpPlacement::Floating => (
                NewPanePlacement::Floating(None),
                ClientTabIndexOrPaneId::ClientId(client_id),
            ),
            // The tab is named rather than left as "wherever the client is": a warp's
            // placement is the user's explicit choice, and `agent_pane_target` re-routes a
            // client-relative agent pane out of a crowded tab while it honours a named one.
            // Resolved now, on the screen thread, it is still the tab the client is looking at.
            WarpPlacement::FocusedTab => (
                NewPanePlacement::Tiled {
                    direction: None,
                    borderless: Some(payload.borderless),
                },
                match self.tab_a_client_is_looking_at(client_id) {
                    Some(tab_index) => ClientTabIndexOrPaneId::TabIndex(tab_index),
                    None => ClientTabIndexOrPaneId::ClientId(client_id),
                },
            ),
            WarpPlacement::NewTab => {
                let tab_index =
                    self.new_tab_for_adopted_pane(invoked_with.clone(), client_id, true)?;
                (
                    NewPanePlacement::Tiled {
                        direction: None,
                        borderless: Some(payload.borderless),
                    },
                    ClientTabIndexOrPaneId::TabIndex(tab_index),
                )
            },
        };
        self.bus
            .senders
            .send_to_screen(ScreenInstruction::NewPane(
                pane_id,
                payload.title.clone(),
                hold_for_command,
                invoked_with,
                placement,
                false,
                tab_target,
                None,
                false,
            ))
            .with_context(err_context)?;
        // Checked straight after `NewPane`, before the replayed exit queued below
        // can close the pane legitimately and make it look unplaced.
        if let Some(finish) = finish.take() {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::AdoptedPaneFinished(finish))
                .with_context(err_context)?;
        }

        if let Some(client_id) = focus_client {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::FocusPaneWithId(
                    pane_id, false, false, client_id, None,
                ))
                .with_context(err_context)?;
        }

        if let Some(caption) = payload.caption {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::SetPaneCaption(pane_id, caption))
                .with_context(err_context)?;
        }

        // The transferred screen first, then whatever the supervisor caught during
        // the hand-off, then live output. `NewPane` is ahead of these on the same
        // channel, and `pending_events_waiting_for_pane` would buffer them anyway.
        let replay = warp_replay_with_theme(
            payload.contents.take(),
            payload.agent_theme.take(),
            payload.bracketed_paste,
            buffered,
        );
        self.bus
            .senders
            .send_to_screen(ScreenInstruction::PtyBytes(terminal_id, replay))
            .with_context(err_context)?;
        // A supervisor's replayed exit must override the agent's pre-handoff state.
        // Already-held panes instead restore their metadata after held state below.
        if payload.held.is_none()
            && let Some(agent) = payload.agent.take()
        {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::SetPaneAgent(pane_id, Some(agent), None))
                .with_context(err_context)?;
        }
        if let Some(exit_status) = child_exit_status {
            let hold_on_close = cmd.hold_on_close;
            let instruction = if let Some(held) = payload.held {
                ScreenInstruction::RestoreHeldPane(
                    pane_id,
                    yinz_types::data::HeldTerminal {
                        command: cmd,
                        ..held
                    },
                )
            } else {
                terminal_exit_instruction(pane_id, exit_status, cmd, hold_on_close)
            };
            self.bus
                .senders
                .send_to_screen(instruction)
                .with_context(err_context)?;
        }
        if let Some(agent) = payload.agent {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::SetPaneAgent(pane_id, Some(agent), None))
                .with_context(err_context)?;
        }
        if request_redraw {
            if let Some(input) = payload.agent_input {
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::RestoreWarpedAgentInput(pane_id, input))
                    .with_context(err_context)?;
            }
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::RequestPaneRedraw(terminal_id))
                .context("failed to request redraw for adopted pane")
                .non_fatal();
        }
        Ok(())
    }

    /// Runs straight after the `NewPane` that `place_adopted_pane` queued. An
    /// adopted terminal that did not get a pane is given back so it can be
    /// adopted again. The warp hold is released only after this check.
    pub(super) fn finish_adopted_pane(&mut self, finish: yinz_bus::warp::AdoptedPaneFinish) {
        let yinz_bus::warp::AdoptedPaneFinish {
            terminal_id,
            recovery,
            held,
            hold: _hold,
            placed,
        } = finish;
        let pane_id = PaneId::Terminal(terminal_id);
        if self
            .tabs
            .values()
            .any(|tab| tab.get_pane_with_id(pane_id).is_some())
        {
            let _ = placed.send(Ok(()));
            return;
        }
        self.bus
            .senders
            .send_to_pty(PtyInstruction::GiveBackAdoptedTerminal {
                terminal_id,
                held,
                recovery,
                placed: Some(placed),
            })
            .context("failed to give back an unplaced adopted pane")
            .non_fatal();
    }

    /// A tab of its own for an adopted pane, following `break_pane`'s dance. Also what a
    /// spawned agent pane gets when no tab has room for it (`agent_placement`), which is why
    /// the caller says whether the client follows the pane there.
    ///
    /// `ignore_run_instruction` is the load-bearing part: without it the new tab's
    /// layout template also spawns a default shell, and the user gets two panes
    /// where they asked for one.
    ///
    /// The client is seated in the tab only when it follows the pane there, as the
    /// `NewTab` arm in `thread_main` does: `apply_layout` moves the client out of its
    /// source tab only on a focus change, so seating it here for an unfocused spawn would
    /// leave it connected to two tabs at once.
    pub(super) fn new_tab_for_adopted_pane(
        &mut self,
        invoked_with: Option<Run>,
        client_id: ClientId,
        should_change_focus_to_new_tab: bool,
    ) -> Result<TabId> {
        let err_context = || "failed to open a tab for an adopted pane".to_string();
        let default_layout = self.default_layout.clone();
        let tab_index = self.get_new_tab_id();
        let swap_layouts = (
            default_layout.swap_tiled_layouts.clone(),
            default_layout.swap_floating_layouts.clone(),
        );
        let client_for_tab = should_change_focus_to_new_tab.then_some(client_id);
        self.new_tab(tab_index, swap_layouts, None, client_for_tab)?;
        self.assign_tab_group(tab_index, Some(client_id));
        let (mut tiled_panes_layout, floating_panes_layout) = default_layout.new_tab();
        tiled_panes_layout.ignore_run_instruction(invoked_with);
        if let Some(tab) = self.tabs.get_mut(&tab_index) {
            tab.agent_tab_cleanup_allowed = !should_change_focus_to_new_tab;
        }
        let lifetime = self
            .tabs
            .get(&tab_index)
            .context("missing new agent tab")?
            .agent_tab_lifetime
            .clone();
        let completion =
            crate::route::NotificationEnd::for_agent_layout(yinz_bus::route::AgentTabPlacement {
                tab_id: tab_index,
                lifetime: std::sync::Arc::downgrade(&lifetime),
                created: true,
            });
        let dispatched = self
            .bus
            .senders
            .send_to_plugin(PluginInstruction::NewTab(
                None,
                None,
                Some(tiled_panes_layout),
                (!floating_panes_layout.is_empty()).then_some(floating_panes_layout),
                tab_index,
                None,
                false,
                should_change_focus_to_new_tab,
                client_id,
                Some(completion),
            ))
            .with_context(err_context);
        if let Err(error) = dispatched {
            // Nothing was dispatched, so no layout or process can arrive later.
            self.close_tab_by_id(tab_index)
                .context("remove undispatched agent tab")
                .non_fatal();
            return Err(error);
        }
        Ok(tab_index)
    }

    /// Build a tab in this session around supervisors the pty thread has already
    /// adopted.
    ///
    /// Expressed as the ordinary new-tab flow rather than by assembling a `Tab`
    /// here, for the same reason `place_adopted_pane` uses `NewPane`: that flow
    /// already knows how to do all of it. Going out through the plugin thread is
    /// what rebuilds the tab's plugin panes — they arrive as `Run::Plugin` nodes
    /// in the layout and come back with plugin ids attached — and the pty thread
    /// then fills the layout's terminal slots from the queue left for it, rather
    /// than spawning anything.
    ///
    /// The tab's name is set afterwards rather than passed in, because the name
    /// travels with the tab while the layout does not carry one.
    pub(super) fn place_adopted_tab(
        &mut self,
        payload: WarpedTab,
        adopted: AdoptedTabPanes,
        client_id: ClientId,
        is_rollback: bool,
        placement: Option<WarpPlacement>,
        response_channel: Option<crossbeam::channel::Sender<Result<()>>>,
        hold: Option<yinz_bus::warp::PaneWarpHold>,
    ) -> Result<()> {
        let target = self.tab_a_client_is_looking_at(client_id);
        let err_context = || "failed to place an adopted tab".to_string();
        let tab_index = self.get_new_tab_id();
        let default_layout = self.default_layout.clone();
        let swap_layouts = (
            default_layout.swap_tiled_layouts.clone(),
            default_layout.swap_floating_layouts.clone(),
        );
        let mut tiled_layout = payload.tiled_layout;
        tiled_layout.hide_floating_panes = payload.hide_floating_panes;
        tiled_layout.group_focused = false;
        let source_group = if !is_rollback && placement.is_none() {
            payload
                .source_instance_id
                .zip(tiled_layout.tab_group.as_ref().map(|group| group.id))
        } else {
            None
        };
        if let Some(group) = &mut tiled_layout.tab_group {
            group.name = yinz_types::text::clean_string_from_control_and_linebreak(&group.name);
            // Rollback IDs are local; only the first arrival matches a mutable displayed name.
            if !is_rollback {
                if let Some(id) = source_group
                    .as_ref()
                    .and_then(|source| self.warped_tab_groups.get(source).copied())
                    .or_else(|| {
                        self.tab_groups.keys().copied().find(|id| {
                            self.tab_group_name(*id).as_deref() == Some(group.name.as_str())
                        })
                    })
                {
                    group.id = id;
                } else {
                    group.id =
                        u32::try_from(self.next_tab_group_id).context("tab group IDs exhausted")?;
                    self.next_tab_group_id += 1;
                }
            }
            if let Some(existing) = self.tab_groups.get(&group.id) {
                group.name = existing.name.clone();
            }
        }
        self.new_tab(
            tab_index,
            swap_layouts,
            payload.name.clone(),
            Some(client_id),
        )?;
        self.restore_tab_group(tab_index, &tiled_layout);
        if let (Some(source), Some(group)) = (source_group, &tiled_layout.tab_group) {
            self.warped_tab_groups.insert(source, group.id);
        }
        // A name the tab was born with elsewhere is only a fallback here, as it was there;
        // a rename the user made there still holds.
        if let Some(tab) = self.tabs.get_mut(&tab_index) {
            tab.set_user_named(payload.user_named);
        }

        if let Some(relationships) = payload.relationships {
            self.pending_warp_relationships
                .insert(tab_index, relationships);
        }
        // Set the terminals aside for this tab *before* asking for it. The
        // request goes out through the plugin thread and comes back to the pty
        // thread, so anything the user opens in the meantime arrives on that
        // same channel — and would otherwise be the tab that got these panes.
        // Sending this first is what rules that out: it is enqueued before the
        // plugin thread has even been asked, so it cannot arrive late.
        self.bus
            .senders
            .send_to_pty(PtyInstruction::ClaimAdoptedTab {
                tab_index,
                terminals: (
                    Self::adopted_terminals(payload.tiled_panes, adopted.tiled),
                    Self::adopted_terminals(payload.floating_panes, adopted.floating),
                ),
            })
            .with_context(err_context)?;
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::NewTab(
                None,
                None,
                Some(tiled_layout),
                Some(payload.floating_layout),
                tab_index,
                None,
                false,
                true,
                client_id,
                None,
            ))
            .with_context(err_context)?;

        if let Some(placement) = placement {
            self.pending_group_placements.insert(
                tab_index,
                (placement, target, client_id, response_channel, hold),
            );
        }

        Ok(())
    }

    /// Pair each pane's transferred screen with the supervisor that was adopted
    /// for it, ready for the pty thread to replay once the tab exists.
    ///
    /// The two lists are in the same order by construction: the pty thread
    /// adopted them in the order they arrived in, and that order is the one the
    /// layout's slots are filled in.
    fn adopted_terminals(
        panes: Vec<WarpedPane>,
        adopted: Vec<AdoptedPane>,
    ) -> Vec<AdoptedTabTerminal> {
        panes
            .into_iter()
            .zip(adopted)
            .map(|(pane, adopted)| {
                // The transferred screen first, then whatever the supervisor
                // caught during the hand-off — the same order a single adopted
                // pane is replayed in, and the same order it happened in.
                let replay = warp_replay_with_theme(
                    pane.contents,
                    pane.agent_theme,
                    pane.bracketed_paste,
                    adopted.buffered,
                );
                AdoptedTabTerminal {
                    agent_input: pane.agent_input,
                    held: pane.held,
                    terminal_id: adopted.terminal_id,
                    cmd: adopted.cmd,
                    agent: pane.agent,
                    child_exit_status: adopted.child_exit_status,
                    caption: pane.caption,
                    replay,
                }
            })
            .collect()
    }
}

/// Restore emulator mode after the screen dump; buffered PTY output is newer and wins.
#[cfg(test)]
pub(super) fn warp_replay(
    contents: Option<String>,
    bracketed_paste: bool,
    buffered: Vec<u8>,
) -> Vec<u8> {
    warp_replay_with_theme(contents, None, bracketed_paste, buffered)
}

fn warp_replay_with_theme(
    contents: Option<String>,
    metadata: Option<yinz_types::agent::theme::AgentThemeHandoff>,
    bracketed_paste: bool,
    buffered: Vec<u8>,
) -> Vec<u8> {
    let mut replay =
        yinz_terminal::agent_theme::handoff_replay(contents.unwrap_or_default(), metadata);
    replay.extend_from_slice(if bracketed_paste {
        b"\x1b[?2004h"
    } else {
        b"\x1b[?2004l"
    });
    replay.extend(buffered);
    replay
}

/// The title a warped pane arrives *named*, when the user or its program gave it one.
///
/// A default - "Pane #3", or the command it runs - is not one: arriving as a name would make it
/// explicit there, and an explicit title is lent to the tab a lone pane sits in, where it
/// outranks the project. A warped tab would wear "Pane #3" instead of the work in it. The
/// default travels as a caption instead ([`default_caption`]).
fn carried_caption(pane: &dyn Pane) -> Option<String> {
    (pane.has_explicit_title() || pane.custom_title().is_some()).then(|| pane.current_title())
}

/// The default caption a terminal pane was wearing, so it arrives captioned the way it was
/// without that caption becoming its name. A stack's members keep their distinct "Pane #N";
/// the receiving session would otherwise number them from where they land, and collide.
///
/// Plugin panes are restarted and caption themselves.
fn default_caption(pane: &dyn Pane) -> Option<String> {
    (matches!(pane.pid(), PaneId::Terminal(_)) && carried_caption(pane).is_none())
        .then(|| pane.current_title())
}

//! The PTY half of a session update. No command is ever respawned here.
use super::Pty;
use crate::global_async_runtime::get_tokio_runtime as async_runtime;
use std::{collections::HashMap, time::Instant};
use yinz_bus::{
    migration::{PtyRequest, ScreenRequest},
    pty::AdoptedTabPanes,
    screen::ScreenInstruction,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::{data::HeldTerminal, internal_ids::TerminalId};
use yinz_types::{
    pane_id::PaneId,
    session_migration::{MigrationOwnership, MigrationPhase},
};

impl Pty {
    fn preflight_migration(
        &self,
        terminals: &[TerminalId],
        held: &HashMap<TerminalId, HeldTerminal>,
    ) -> Result<()> {
        anyhow::ensure!(
            self.pending_terminal_spawns.is_empty()
                && self.reruns.is_empty()
                && self.adopted_tabs.is_empty(),
            "terminal startup, restart, or tab adoption is still pending; retry once it finishes"
        );
        anyhow::ensure!(self.task_handles.keys().chain(self.terminal_launch_commands.keys()).all(|id|terminals.contains(id)),"a terminal was created outside the captured session layout; retry after placement finishes");
        let os = self
            .bus
            .os_input
            .as_ref()
            .context("missing terminal backend")?;
        for id in terminals {
            if !held.contains_key(id) {
                os.terminal_migration_address(*id)
                    .with_context(|| format!("preflight terminal_{id}"))?;
                anyhow::ensure!(
                    self.reader_controls.contains_key(id),
                    "terminal_{id} has no cooperative output reader"
                );
            }
        }
        Ok(())
    }

    pub(super) fn pause_terminal_readers(
        &mut self,
        terminals: &[TerminalId],
        deadline: Instant,
    ) -> Result<()> {
        let mut controls: Vec<_> = terminals
            .iter()
            .filter_map(|id| self.reader_controls.get(id).cloned())
            .collect();
        let already_paused: Vec<_> = controls.iter().map(|control| control.pause()).collect();
        let result = async_runtime().block_on(async {
            tokio::time::timeout_at(deadline.into(), async {
                for control in &mut controls {
                    control.wait_until_paused().await?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await
            .context("wait for terminal output to quiesce")?
        });
        if result.is_err() {
            // A pane transfer must not resume unrelated readers awaiting adoption,
            // or a selected reader that was already paused before this request.
            for (control, was_paused) in controls.iter().zip(already_paused) {
                if !was_paused {
                    control.resume();
                }
            }
        }
        result
    }

    fn resume_migration_readers(&mut self) {
        self.migration_readers_paused = false;
        for control in self.reader_controls.values() {
            control.resume();
        }
    }

    fn release_migration_owned(&mut self, id: TerminalId) -> Result<()> {
        let owned = self
            .bus
            .os_input
            .as_ref()
            .context("missing migration backend")?
            .owns_terminal_for_migration(id)
            .context("inspect destination ownership")?;
        if owned {
            self.pause_terminal_readers(
                &[id],
                Instant::now() + std::time::Duration::from_secs(10),
            )?;
            self.stop_readers_for_warp(&[id]);
            let release = self
                .bus
                .os_input
                .as_ref()
                .context("missing migration backend")?
                .release_terminal_for_warp(id, std::time::Duration::from_secs(10));
            let retained = self
                .bus
                .os_input
                .as_ref()
                .context("missing migration backend")?
                .owns_terminal_for_migration(id)
                .context("verify destination handback")?;
            anyhow::ensure!(
                !retained,
                "terminal_{id} remains owned after handback: {release:?}"
            );
        }
        self.forget_released_terminal(id);
        Ok(())
    }

    fn record_migration_handback(&mut self, id: TerminalId) -> Result<()> {
        if let Some((path, snapshot)) = self.migration_recovery_snapshot.as_mut() {
            for (_, ownership) in &mut snapshot.ownership {
                if matches!(ownership, MigrationOwnership::Destination { terminal_id } if *terminal_id == id)
                {
                    *ownership = MigrationOwnership::Released;
                }
            }
            yinz_bus::migration::save_snapshot(path, snapshot)
                .context("record destination terminal handback")?;
        }
        Ok(())
    }

    pub(super) fn session_migration(&mut self, request: PtyRequest) {
        match request {
            PtyRequest::Prepare {
                inventory,
                terminals,
                held,
                reply,
            } => {
                let result = self
                    .preflight_migration(&terminals, &held)
                    .map(|()| inventory)
                    .map_err(|e| format!("{e:#}"));
                let _ = reply.send(result);
            },
            PtyRequest::Pause {
                operation,
                terminals,
                held,
                deadline,
            } => {
                let result = (|| {
                    self.preflight_migration(&terminals, &held)?;
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "session migration deadline expired before pausing readers"
                    );
                    for id in held.keys() {
                        self.drain_held_reader_for_warp(*id, deadline)
                            .context("drain held terminal final output")?;
                    }
                    self.pause_terminal_readers(&terminals, deadline)?;
                    let os = self
                        .bus
                        .os_input
                        .as_ref()
                        .context("missing terminal backend")?;
                    terminals
                        .iter()
                        .map(|id| {
                            let address = if let Some(held) = held.get(id) {
                                crate::os_input_output::ReleasedPane::Held(held.clone())
                            } else {
                                os.terminal_migration_address(*id).with_context(|| {
                                    format!("read terminal_{id} recovery address")
                                })?
                            };
                            Ok((*id, address))
                        })
                        .collect::<Result<Vec<_>>>()
                })();
                if result.is_err() {
                    self.resume_migration_readers();
                }
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::SessionMigration(
                        ScreenRequest::ReadersPaused {
                            operation,
                            result: result.map_err(|e| format!("{e:#}")),
                        },
                    ))
                    .context("report paused migration readers")
                    .non_fatal();
            },
            PtyRequest::Release {
                operation,
                terminals,
                held,
                deadline,
                path,
            } => {
                // Readers have acknowledged that every byte they consumed was sent
                // to Screen, and Screen durably recorded the matching snapshot and
                // replay offsets before issuing this instruction.
                let mut results = Vec::new();
                let mut document = yinz_bus::migration::load_snapshot(&path, &operation);
                let mut stopped = false;
                for id in terminals {
                    let command = held
                        .get(&id)
                        .map(|h| h.command.clone())
                        .unwrap_or_else(|| super::warp::warp_restart_command(self, id));
                    let result = (|| -> Result<_> {
                        anyhow::ensure!(!stopped, "release stopped after an earlier ownership error; terminal remains with source");
                        let document = document
                            .as_mut()
                            .map_err(|error| anyhow!("load release journal: {error:#}"))?;
                        document
                            .record_ownership(PaneId::Terminal(id), MigrationOwnership::Releasing);
                        yinz_bus::migration::save_snapshot(&path, document)
                            .context("record pending terminal release")?;
                        let result = if let Some(held) = held.get(&id) {
                            Ok(crate::os_input_output::ReleasedPane::Held(held.clone()))
                        } else {
                            self.release_terminal_for_warp(id, deadline)
                                .with_context(|| {
                                    format!("release terminal_{id} during session update")
                                })
                        };
                        document.record_ownership(
                            PaneId::Terminal(id),
                            if result.is_ok() {
                                MigrationOwnership::Released
                            } else {
                                MigrationOwnership::Uncertain
                            },
                        );
                        yinz_bus::migration::save_snapshot(&path, document)
                            .context("record terminal release outcome")?;
                        result
                    })();
                    stopped |= result.is_err();
                    results.push((
                        id,
                        result
                            .map(|pane| yinz_bus::warp::ReleasedPaneForWarp {
                                pane,
                                run_command: command,
                            })
                            .map_err(|e| format!("{e:#}")),
                    ));
                }
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::SessionMigration(
                        ScreenRequest::Released { operation, results },
                    ))
                    .context("report session release ownership")
                    .non_fatal();
            },
            PtyRequest::ReconcileSource {
                operation,
                path,
                client,
                reply,
            } => {
                let result = (|| -> Result<_> {
                    let mut snapshot = yinz_bus::migration::load_snapshot(&path, &operation)?;
                    let terminals: Vec<_> = snapshot.tabs.iter().flat_map(|tab| {
                        tab.terminal_states.iter().zip(tab.payload.tiled_panes.iter().chain(&tab.payload.floating_panes))
                            .map(|((id,_),pane)|(*id,pane.held.is_some()))
                    }).collect();
                    for (pane, held) in terminals {
                        let PaneId::Terminal(id) = pane else { return Err(anyhow!("nonterminal source recovery address")); };
                        let owned = !held && self.bus.os_input.as_ref().context("missing source recovery backend")?
                            .owns_terminal_for_migration(id).context("inspect source terminal ownership")?;
                        if owned {
                            snapshot.record_ownership(pane, MigrationOwnership::Releasing);
                            yinz_bus::migration::save_snapshot(&path, &snapshot).context("record source reconciliation release")?;
                            // The original pause barrier already completed. Bypass
                            // ordinary warp preflight: a disconnected remote must
                            // drop its stale transport so its token can be resumed.
                            self.stop_readers_for_warp(&[id]);
                            let result = self.bus.os_input.as_ref().context("missing source recovery backend")?
                                .release_terminal_for_warp(id, std::time::Duration::from_secs(10));
                            let retained = self.bus.os_input.as_ref().context("missing source recovery backend")?
                                .owns_terminal_for_migration(id).context("verify source release ownership")?;
                            if retained {
                                snapshot.record_ownership(pane, MigrationOwnership::Uncertain);
                                yinz_bus::migration::save_snapshot(&path, &snapshot).context("record retained source ownership")?;
                                return Err(anyhow!("terminal_{id} is still owned by the source after reconciliation: {result:?}"));
                            }
                            // No local handle survives an uncertain remote ack.
                            // Its broker still arbitrates exclusive Resume; a
                            // disconnected network can refuse adoption safely.
                        }
                        if !held { self.forget_released_terminal(id); }
                        snapshot.record_ownership(pane, MigrationOwnership::Released);
                        yinz_bus::migration::save_snapshot(&path, &snapshot).context("record reconciled terminal ownership")?;
                    }
                    snapshot.phase = MigrationPhase::Released;
                    snapshot.errors.clear();
                    yinz_bus::migration::save_snapshot(&path, &snapshot).context("record recovered source release")?;
                    Ok(Box::new(snapshot))
                })().map_err(|error|format!("{error:#}"));
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::SessionMigration(
                        ScreenRequest::SourceReconciled {
                            operation,
                            path,
                            client,
                            reply,
                            result,
                        },
                    ))
                    .context("report reconciled source migration")
                    .non_fatal();
            },
            PtyRequest::Resume => {
                self.migration_owned = None;
                self.migration_recovery_snapshot = None;
                self.resume_migration_readers();
            },
            PtyRequest::Abort { operation, reply } => {
                let result = (|| {
                    // Do not give back a local supervisor until every consumed
                    // replay byte is durable. An earlier disk error can be
                    // retried without losing the in-memory recovery payload.
                    if let Some((path, snapshot)) = &self.migration_recovery_snapshot {
                        anyhow::ensure!(snapshot.operation == operation, "stale destination abort");
                        yinz_bus::migration::save_snapshot(path, snapshot)
                            .context("retain consumed local output before destination handback")?;
                    }
                    let Some((owner, panes)) = self.migration_owned.take() else {
                        return Ok(());
                    };
                    if owner != operation {
                        self.migration_owned = Some((owner, panes));
                        return Err(anyhow!("stale destination abort"));
                    }
                    let mut uncertain = Vec::new();
                    let mut errors = Vec::new();
                    for (id, held) in panes {
                        if held {
                            self.give_back_adopted(id, true);
                        } else if let Err(error) = self.release_migration_owned(id) {
                            errors.push(format!("terminal_{id}: {error:#}"));
                            uncertain.push((id, held));
                            continue;
                        }
                        if let Err(error) = self.record_migration_handback(id) {
                            errors.push(format!("record terminal_{id} handback: {error:#}"));
                            uncertain.push((id, held));
                        }
                    }
                    if !uncertain.is_empty() {
                        self.migration_owned = Some((owner, uncertain));
                        return Err(anyhow!(
                            "destination handback incomplete: {}",
                            errors.join("; ")
                        ));
                    }
                    self.adopted_tabs.clear();
                    Ok(())
                })()
                .map_err(|e| format!("{e:#}"));
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::SessionMigration(
                        ScreenRequest::Aborted {
                            operation,
                            result,
                            reply,
                        },
                    ))
                    .context("acknowledge destination handback")
                    .non_fatal();
            },
            PtyRequest::Adopt {
                mut snapshot,
                client,
                path,
                reply,
            } => {
                self.migration_readers_paused = true;
                let mut adopted = Vec::new();
                let mut ownership = Vec::new();
                let result: Result<Vec<AdoptedTabPanes>> = (|| {
                    for tab_index in 0..snapshot.tabs.len() {
                        let tab = &snapshot.tabs[tab_index];
                        let sources = [
                            tab.payload.tiled_panes.clone(),
                            tab.payload.floating_panes.clone(),
                        ];
                        let source_ids: Vec<_> =
                            tab.terminal_states.iter().map(|(id, _)| *id).collect();
                        anyhow::ensure!(
                            source_ids.len() == sources.iter().map(Vec::len).sum::<usize>(),
                            "migration terminal identity count changed"
                        );
                        let mut source_ids = source_ids.into_iter();
                        let mut panes = AdoptedTabPanes {
                            tiled: vec![],
                            floating: vec![],
                        };
                        for (sources, destination) in
                            sources.iter().zip([&mut panes.tiled, &mut panes.floating])
                        {
                            for source in sources {
                                let source_id = source_ids
                                    .next()
                                    .context("missing migration terminal identity")?;
                                snapshot.record_ownership(source_id, MigrationOwnership::Adopting);
                                yinz_bus::migration::save_snapshot(&path, &snapshot)
                                    .context("record pending destination adoption")?;
                                let PaneId::Terminal(requested) = source_id else {
                                    return Err(anyhow!("migration source slot is not a terminal"));
                                };
                                let mut pane = self
                                    .adopt_terminal_with_id(source, Some(requested))
                                    .context("adopt session terminal")?;
                                ownership.push((pane.terminal_id, source.held.is_some()));
                                snapshot.record_ownership(
                                    source_id,
                                    MigrationOwnership::Destination {
                                        terminal_id: pane.terminal_id,
                                    },
                                );
                                if source.remote.is_none() && source.held.is_none() {
                                    let replay = &mut snapshot.tabs[tab_index].local_replay;
                                    if let Some((_, bytes)) =
                                        replay.iter_mut().find(|(id, _)| *id == source_id)
                                    {
                                        bytes.append(&mut pane.buffered);
                                        pane.buffered = bytes.clone();
                                    } else if !pane.buffered.is_empty() {
                                        replay.push((source_id, pane.buffered.clone()));
                                    }
                                }
                                yinz_bus::migration::save_snapshot(&path, &snapshot).context(
                                    "retain detached output and destination ownership before further adoption",
                                )?;
                                destination.push(pane);
                            }
                        }
                        adopted.push(panes);
                    }
                    Ok(adopted)
                })();
                if result.is_err() {
                    // No output has been applied to a destination emulator: retain
                    // original replay offsets so source recovery replays that output.
                    let mut uncertain = Vec::new();
                    let mut remaining = Vec::new();
                    let replay_saved = yinz_bus::migration::save_snapshot(&path, &snapshot).is_ok();
                    for (id, held) in &ownership {
                        let (id, held) = (*id, *held);
                        if !replay_saved {
                            remaining.push((id, held));
                        } else if held {
                            self.give_back_adopted(id, true);
                        } else if let Err(error) = self.release_migration_owned(id) {
                            uncertain.push(format!("release adopted terminal_{id}: {error:#}"));
                            remaining.push((id, held));
                            continue;
                        }
                        if replay_saved {
                            for (_, owner) in &mut snapshot.ownership {
                                if matches!(owner, MigrationOwnership::Destination { terminal_id } if *terminal_id == id)
                                {
                                    *owner = MigrationOwnership::Released;
                                }
                            }
                            if let Err(error) = yinz_bus::migration::save_snapshot(&path, &snapshot)
                            {
                                uncertain.push(format!(
                                    "record adopted terminal_{id} handback: {error:#}"
                                ));
                                remaining.push((id, held));
                            }
                        }
                    }
                    ownership = remaining;
                    snapshot.phase = yinz_types::session_migration::MigrationPhase::Recovery;
                    snapshot.errors.extend(uncertain);
                    if !replay_saved {
                        snapshot.errors.push("consumed local output is retained in the destination process; retry destination abort after restoring snapshot storage".into());
                    }
                    if let Err(error) = yinz_bus::migration::save_snapshot(&path, &snapshot) {
                        log::error!(
                            "update {}: could not record failed adoption: {error:#}",
                            snapshot.operation
                        );
                    }
                }
                self.migration_owned = Some((snapshot.operation.clone(), ownership));
                self.migration_recovery_snapshot = Some((path.clone(), (*snapshot).clone()));
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::SessionMigration(
                        ScreenRequest::Adopted {
                            snapshot,
                            result: result.map_err(|e| format!("{e:#}")),
                            client,
                            path,
                            reply,
                        },
                    ))
                    .context("report session adoption")
                    .non_fatal();
            },
        }
    }
}

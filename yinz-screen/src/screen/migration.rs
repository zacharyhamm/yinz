//! Whole-session migration. The source keeps both its screen thread and its
//! recovery snapshot until ownership has been acknowledged by the destination.
use super::{PreparedTabWarp, Screen};
use std::{collections::HashMap, path::PathBuf, time::Instant};
use yinz_bus::{
    migration::{self, PtyRequest, Reply, ScreenRequest},
    pty::PtyInstruction,
    screen::ScreenInstruction,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    data::{HeldTerminal, WarpedPane, WarpedTab},
    internal_ids::{ClientId, TabId, TerminalId},
    pane_id::PaneId,
    session_migration::*,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Pausing,
    Releasing,
    Released,
    Adopting,
    Adopted,
    Finishing,
    Repairing,
    Recovery,
    Recovering,
}

pub(super) struct MigrationState {
    operation: String,
    path: PathBuf,
    stage: Stage,
    snapshot: SessionMigrationSnapshot,
    prepared: Vec<(TabId, PreparedTabWarp)>,
    held: HashMap<TerminalId, HeldTerminal>,
    deadline: Instant,
    release_reply: Option<Reply<SessionMigrationSnapshot>>,
    adoption_reply: Option<Reply<SessionMigrationMap>>,
    finish_replies: Vec<Reply<()>>,
    pending_tabs: HashMap<TabId, MigrationTab>,
    replay_tabs: std::collections::HashSet<TabId>,
}

type PreparedSessionMigration = (
    SessionMigrationInventory,
    Vec<(TabId, PreparedTabWarp)>,
    Vec<TerminalId>,
    HashMap<TerminalId, HeldTerminal>,
);

impl Screen {
    pub(super) fn migration_blocks_normal_work(&self) -> bool {
        self.bus
            .senders
            .session_update_quiesced
            .load(std::sync::atomic::Ordering::Acquire)
    }
    pub(super) fn migration_reports_suspended(&self) -> bool {
        self.bus
            .senders
            .session_update_quiesced
            .load(std::sync::atomic::Ordering::Acquire)
            && !self
                .migration
                .as_ref()
                .is_some_and(|state| state.stage == Stage::Finishing)
    }

    pub(super) fn migration_inventory(
        &mut self,
        client: ClientId,
    ) -> Result<PreparedSessionMigration> {
        anyhow::ensure!(
            self.watcher_clients.is_empty(),
            "disconnect read-only watchers and retry the session update"
        );
        anyhow::ensure!(
            self.pending_warps.is_empty()
                && self.pending_tab_warps.is_empty()
                && self.pending_pane_groups.is_empty()
                && self.pending_group_placements.is_empty()
                && self.in_flight_pane_warps == 0,
            "a pane transfer is already active"
        );
        anyhow::ensure!(
            !self.has_pending_tabs() && self.pending_arrangement.is_none(),
            "a layout is still being applied; retry after it finishes"
        );
        anyhow::ensure!(
            self.resolving_plugins.is_empty() && self.animating_plugin_loads.is_empty(),
            "a plugin is still loading; retry after it finishes"
        );
        let mut ordered: Vec<_> = self
            .tabs
            .values()
            .map(|tab| (tab.position, tab.id))
            .collect();
        ordered.sort_by_key(|(position, _)| *position);
        let mut prepared = Vec::new();
        let mut tabs = Vec::new();
        let mut terminals = Vec::new();
        let mut held = HashMap::new();
        let mut remote_hosts = std::collections::BTreeSet::new();
        for (_, id) in ordered {
            let (_, ids, tab) = self
                .check_tab_can_be_warped(Some(id), client)
                .map_err(|e| anyhow!("{e}"))?;
            let current = self.tabs.get(&id).context("migration tab disappeared")?;
            current
                .migration_state()
                .map_err(|e| anyhow!("tab {id}: {e}"))?;
            for pane_id in current.get_all_pane_ids() {
                anyhow::ensure!(tab.slots.contains(&pane_id),"pane {pane_id:?} has no transferable layout slot; close transient overlays before updating");
            }
            for pane_id in &tab.slots {
                let pane = current
                    .get_pane_with_id(*pane_id)
                    .context("missing migration pane")?;
                if let PaneId::Terminal(terminal) = pane_id {
                    pane.migration_state()
                        .map_err(|e| anyhow!("terminal_{terminal} cannot be migrated: {e}"))?;
                    if let Some(state) = pane.held_for_warp() {
                        held.insert(*terminal, state);
                    }
                    if let Some(host) = self
                        .bus
                        .os_input
                        .as_ref()
                        .and_then(|os| os.remote_pane_host(*terminal))
                    {
                        remote_hosts.insert(host);
                    }
                }
            }
            tabs.push(TabInventory {
                tab_id: id,
                title: current.name.clone(),
                panes: tab.slots.clone(),
            });
            terminals.extend(ids);
            prepared.push((id, tab));
        }
        anyhow::ensure!(!tabs.is_empty(), "there are no tabs to migrate");
        let clients = self
            .connected_clients
            .borrow()
            .iter()
            .map(|id| ClientFocus {
                client_id: *id,
                tab_id: self.active_tab_ids.get(id).copied(),
                pane_id: self.get_active_pane_id(id),
                tab_history: self.tab_history.get(id).cloned().unwrap_or_default(),
                pane_history: self.pane_history.get(id).cloned().unwrap_or_default(),
                mode_info: self
                    .mode_info
                    .get(id)
                    .unwrap_or(&self.default_mode_info)
                    .clone(),
                marked_panes: self
                    .current_pane_group
                    .borrow()
                    .clone_inner()
                    .remove(id)
                    .unwrap_or_default(),
            })
            .collect();
        Ok((
            SessionMigrationInventory {
                format: SESSION_MIGRATION_FORMAT,
                session_name: self.session_name.clone(),
                instance_id: self.instance_id.to_string(),
                tabs,
                clients,
                remote_hosts: remote_hosts.into_iter().collect(),
                global_last_active_tab: self.global_last_active_tab_id,
                tab_group_preferences: self
                    .tab_groups
                    .iter()
                    .map(|(id, group)| (*id, group.preferred))
                    .collect(),
                paneless_plugins: self
                    .paneless_plugins
                    .iter()
                    .filter(|(id, _)| !self.failed_paneless_plugins.contains(id))
                    .map(|(_, run)| run.clone())
                    .collect(),
            },
            prepared,
            terminals,
            held,
        ))
    }

    /// Every instruction that can change structure, input, focus, or pane state
    /// waits behind the handoff. Output drains up to the cooperative reader
    /// barrier; placement messages are admitted only while adopting.
    pub(super) fn defer_for_migration(
        &mut self,
        event: ScreenInstruction,
    ) -> Option<ScreenInstruction> {
        let Some(state) = &self.migration else {
            return Some(event);
        };
        // Finish is the coordinator's irreversible commit boundary. Plugin
        // startup callbacks must be able to complete their normal host work
        // before the readiness acknowledgement; old deferred source events
        // remain separate and are discarded when commit completes.
        if matches!(state.stage, Stage::Finishing | Stage::Repairing) {
            return Some(event);
        }
        let allowed = matches!(
            &event,
            ScreenInstruction::SessionMigration(_)
                | ScreenInstruction::Exit
                | ScreenInstruction::PtyBytes(..)
                | ScreenInstruction::Render
                | ScreenInstruction::RenderToClients
                | ScreenInstruction::RenderWriterReady
                | ScreenInstruction::RemoveClient(..)
                | ScreenInstruction::Notify(..)
                | ScreenInstruction::DumpLayoutToPlugin { .. }
                | ScreenInstruction::GetCellPixelSize { .. }
                | ScreenInstruction::GetFocusedPaneInfo { .. }
                | ScreenInstruction::GetPaneInfo { .. }
                | ScreenInstruction::GetTabInfo { .. }
                | ScreenInstruction::GetPaneScrollback { .. }
                | ScreenInstruction::ListPanes { .. }
                | ScreenInstruction::ListTabs { .. }
                | ScreenInstruction::ListNotifications { .. }
                | ScreenInstruction::GetCurrentTabInfo { .. }
                | ScreenInstruction::ListClientsToPlugin(..)
                | ScreenInstruction::ReportCellPixelSize(..)
        ) || matches!(state.stage, Stage::Adopted | Stage::Finishing)
            && matches!(
                &event,
                ScreenInstruction::AddClient(..)
                    | ScreenInstruction::RecomputeTabSize(..)
                    | ScreenInstruction::RequestRepaint(..)
            )
            || matches!(state.stage, Stage::Adopting | Stage::Finishing)
                && matches!(
                    &event,
                    ScreenInstruction::ApplyLayout(..)
                        | ScreenInstruction::AddPlugin(..)
                        | ScreenInstruction::PluginBytes(..)
                        | ScreenInstruction::SetSelectable(..)
                        | ScreenInstruction::SetMouseSelectionSupport(..)
                        | ScreenInstruction::ShowPluginCursor(..)
                        | ScreenInstruction::SetPaneCaption(..)
                        | ScreenInstruction::SetPaneAgent(..)
                        | ScreenInstruction::RestoreHeldPane(..)
                        | ScreenInstruction::RestoreWarpedAgentInput(..)
                        | ScreenInstruction::RequestPaneRedraw(..)
                        | ScreenInstruction::HoldPane(..)
                        | ScreenInstruction::LayoutSpawnFailed(..)
                );
        if allowed {
            Some(event)
        } else {
            self.migration_deferred.push_back(event);
            None
        }
    }

    pub(super) fn next_migration_event(&mut self) -> Option<ScreenInstruction> {
        if !self.migration_blocks_normal_work() {
            self.migration_deferred.pop_front()
        } else {
            None
        }
    }

    fn migration_tab_snapshot(
        &self,
        id: TabId,
        prepared: &PreparedTabWarp,
        addresses: &HashMap<TerminalId, crate::os_input_output::ReleasedPane>,
    ) -> Result<MigrationTab> {
        let tab = self
            .tabs
            .get(&id)
            .context("missing source tab during snapshot")?;
        let mut terminal_states = Vec::new();
        let mut serialize = |pane_id: &PaneId| -> Result<WarpedPane> {
            let PaneId::Terminal(terminal) = pane_id else {
                return Err(anyhow!("plugin occupies a terminal migration slot"));
            };
            let pane = tab
                .get_pane_with_id(*pane_id)
                .context("missing source terminal during snapshot")?;
            let state = pane
                .migration_state()
                .map_err(|e| anyhow!("terminal_{terminal}: {e}"))?;
            terminal_states.push((*pane_id, state));
            let address = Self::warp_address(
                addresses
                    .get(terminal)
                    .context("missing terminal recovery address")?
                    .clone(),
            )
            .map_err(|e| anyhow!("{e}"))?;
            Ok(WarpedPane {
                held: address.held,
                host_id: address.host_id,
                control_socket: address.control_socket,
                remote: address.remote,
                caption: (!pane.has_explicit_title() && pane.custom_title().is_none())
                    .then(|| pane.current_title()),
                bracketed_paste: pane.supports_bracketed_paste(),
                agent: self
                    .tabs
                    .get(&id)
                    .and_then(|tab| tab.get_pane_with_id(*pane_id))
                    .and_then(|p| p.agent_info().cloned()),
                agent_input: self.warped_agent_input(*pane_id),
                ..Default::default()
            })
        };
        let tiled_panes = prepared
            .tiled_panes
            .iter()
            .map(&mut serialize)
            .collect::<Result<_>>()?;
        let floating_panes = prepared
            .floating_panes
            .iter()
            .map(&mut serialize)
            .collect::<Result<_>>()?;
        Ok(MigrationTab {
            source_tab_id: id,
            slots: prepared.slots.clone(),
            terminal_states,
            local_replay: vec![],
            state: tab.migration_state().map_err(|e| anyhow!("{e}"))?,
            payload: WarpedTab {
                source_instance_id: Some(self.instance_id.to_string()),
                relationships: prepared.relationships.clone(),
                name: prepared.name.clone(),
                user_named: prepared.user_named,
                tiled_layout: prepared.tiled_layout.clone(),
                floating_layout: prepared.floating_layout.clone(),
                tiled_panes,
                floating_panes,
                hide_floating_panes: prepared.hide_floating_panes,
            },
        })
    }

    fn remove_migration_tabs(&mut self, prepared: &[(TabId, PreparedTabWarp)]) {
        for (id, prepared) in prepared {
            if let Some(tab) = self.tabs.get_mut(id) {
                let removed = tab.extract_warp_panes(&prepared.slots);
                for pane in removed {
                    // The process is already released. Never send ClosePane for
                    // a live terminal: that would terminate its supervisor.
                    let pane_id = pane.pid();
                    if matches!(pane_id, PaneId::Plugin(_)) || pane.is_held() {
                        self.bus
                            .senders
                            .send_to_pty(PtyInstruction::ClosePane(pane_id, None))
                            .context("retire source pane without a process")
                            .non_fatal();
                    }
                    self.clear_nested_guest(pane_id);
                }
            }
            self.empty_and_close_tab(*id);
        }
    }

    pub(super) fn session_migration(&mut self, request: ScreenRequest) {
        match request {
            ScreenRequest::Prepare {
                operation,
                client,
                reply,
            } => {
                let result: Result<()> = (|| {
                    migration::validate_operation(&operation)?;
                    anyhow::ensure!(
                        self.migration.is_none(),
                        "this session already has an active migration"
                    );
                    let (inventory, _, terminals, held) = self.migration_inventory(client)?;
                    self.bus
                        .senders
                        .send_to_pty(PtyInstruction::SessionMigration(PtyRequest::Prepare {
                            inventory,
                            terminals,
                            held,
                            reply: reply.clone(),
                        }))
                        .context("preflight every terminal")
                })();
                if let Err(e) = result {
                    let _ = reply.send(Err(format!("{e:#}")));
                }
            },
            ScreenRequest::Release {
                operation,
                snapshot,
                client,
                deadline,
                reply,
            } => {
                let result: Result<()> = (|| {
                    migration::validate_operation(&operation)?;
                    if let Some(state) = &self.migration {
                        anyhow::ensure!(
                            state.operation == operation,
                            "another session migration is active"
                        );
                        if state.stage == Stage::Released {
                            let _ = reply.send(Ok(state.snapshot.clone()));
                            return Ok(());
                        }
                        return Err(anyhow!(
                            "session migration is already in progress; inspect its journal"
                        ));
                    }
                    let (inventory, prepared, _, held) = self.migration_inventory(client)?;
                    let document = SessionMigrationSnapshot {
                        format: SESSION_MIGRATION_FORMAT,
                        operation: operation.clone(),
                        inventory,
                        phase: MigrationPhase::Prepared,
                        tabs: vec![],
                        mapping: Default::default(),
                        errors: vec![],
                        ownership: vec![],
                    };
                    migration::save_snapshot(&snapshot, &document)?;
                    self.migration = Some(MigrationState {
                        operation: operation.clone(),
                        path: snapshot,
                        stage: Stage::Pausing,
                        snapshot: document,
                        prepared,
                        held: held.clone(),
                        deadline,
                        release_reply: Some(reply.clone()),
                        adoption_reply: None,
                        finish_replies: vec![],
                        pending_tabs: HashMap::new(),
                        replay_tabs: Default::default(),
                    });
                    self.bus
                        .senders
                        .session_update_quiesced
                        .store(true, std::sync::atomic::Ordering::Release);
                    let (tx, rx) = std::sync::mpsc::channel();
                    if let Err(error) = self
                        .bus
                        .senders
                        .send_to_plugin(yinz_bus::plugins::PluginInstruction::QuiesceForUpdate {
                            operation: operation.clone(),
                            reply: tx,
                        })
                        .context("quiesce plugin persistence")
                    {
                        self.cancel_migration_preparation(format!("{error:#}"));
                        return Ok(());
                    }
                    let senders = self.bus.senders.clone();
                    std::thread::spawn(move || {
                        let result = rx
                            .recv_timeout(std::time::Duration::from_secs(65))
                            .map_err(|e| format!("wait for plugin persistence: {e}"))
                            .and_then(|reply| reply);
                        senders
                            .send_to_screen(ScreenInstruction::SessionMigration(
                                ScreenRequest::PluginsPaused { operation, result },
                            ))
                            .context("report plugin quiescence")
                            .non_fatal();
                    });
                    Ok(())
                })();
                if let Err(e) = result {
                    let _ = reply.send(Err(format!("{e:#}")));
                }
            },
            ScreenRequest::PluginsPaused { operation, result } => {
                let Some(mut state) = self.migration.take() else {
                    return;
                };
                if state.operation != operation || state.stage != Stage::Pausing {
                    self.migration = Some(state);
                    return;
                }
                match result {
                    Ok(()) => {
                        let terminals = state
                            .prepared
                            .iter()
                            .flat_map(|(_, tab)| tab.tiled_panes.iter().chain(&tab.floating_panes))
                            .filter_map(|id| match id {
                                PaneId::Terminal(id) => Some(*id),
                                _ => None,
                            })
                            .collect();
                        let instruction = PtyRequest::Pause {
                            operation,
                            terminals,
                            held: state.held.clone(),
                            deadline: state.deadline,
                        };
                        self.migration = Some(state);
                        if let Err(error) = self
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::SessionMigration(instruction))
                            .context("pause session output readers")
                        {
                            self.cancel_migration_preparation(format!("{error:#}"));
                        }
                    },
                    Err(error) => {
                        self.resume_migration_plugins(&operation);
                        self.bus
                            .senders
                            .session_update_quiesced
                            .store(false, std::sync::atomic::Ordering::Release);
                        if let Some(reply) = state.release_reply.take() {
                            let _ = reply.send(Err(error));
                        }
                    },
                }
            },
            ScreenRequest::ReadersPaused { operation, result } => {
                let Some(state) = self.migration.as_ref() else {
                    return;
                };
                if state.operation != operation || state.stage != Stage::Pausing {
                    return;
                }
                let (tx, rx) = crossbeam::channel::bounded(1);
                let deadline = state.deadline;
                let senders = self.bus.senders.clone();
                if let Err(error) = senders.send_to_pty_writer(
                    yinz_bus::pty_writer::PtyWriteInstruction::MigrationBarrier {
                        deadline,
                        reply: tx,
                    },
                ) {
                    self.session_migration(ScreenRequest::InputDrained {
                        operation,
                        result: Err(format!("drain migration input: {error:#}")),
                    });
                    return;
                }
                std::thread::spawn(move || {
                    let result = result.and_then(|addresses| {
                        rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                            .map_err(|e| format!("drain migration input: {e}"))
                            .and_then(|r| r)
                            .map(|()| addresses)
                    });
                    senders
                        .send_to_screen(ScreenInstruction::SessionMigration(
                            ScreenRequest::InputDrained { operation, result },
                        ))
                        .context("report drained migration input")
                        .non_fatal();
                });
            },
            ScreenRequest::InputDrained { operation, result } => {
                let Some(mut state) = self.migration.take() else {
                    return;
                };
                if state.operation != operation || state.stage != Stage::Pausing {
                    self.migration = Some(state);
                    return;
                }
                let prepared = (|| {
                    let addresses: HashMap<_, _> =
                        result.map_err(|e| anyhow!("{e}"))?.into_iter().collect();
                    anyhow::ensure!(
                        Instant::now() < state.deadline,
                        "migration expired before terminal ownership changed"
                    );
                    state.snapshot.tabs = state
                        .prepared
                        .iter()
                        .map(|(id, tab)| self.migration_tab_snapshot(*id, tab, &addresses))
                        .collect::<Result<_>>()?;
                    state.snapshot.phase = MigrationPhase::Releasing;
                    state.snapshot.ownership = addresses
                        .keys()
                        .map(|id| (PaneId::Terminal(*id), MigrationOwnership::Source))
                        .collect();
                    anyhow::ensure!(
                        Instant::now() < state.deadline,
                        "migration expired while serializing its terminals"
                    );
                    migration::save_snapshot(&state.path, &state.snapshot)?;
                    Ok(addresses.keys().copied().collect())
                })();
                match prepared {
                    Ok(terminals) => {
                        state.stage = Stage::Releasing;
                        let instruction = PtyRequest::Release {
                            operation,
                            terminals,
                            held: state.held.clone(),
                            deadline: state.deadline,
                            path: state.path.clone(),
                        };
                        self.migration = Some(state);
                        if let Err(error) = self
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::SessionMigration(instruction))
                            .context("release durably recorded session terminals")
                        {
                            // Sending failed, so no terminal ownership changed.
                            // Revert the journal before allowing source actions.
                            if let Some(state) = self.migration.as_mut() {
                                state.stage = Stage::Pausing;
                                state.snapshot.phase = MigrationPhase::Prepared;
                                migration::save_snapshot(&state.path, &state.snapshot)
                                    .context("record untouched source after send failure")
                                    .non_fatal();
                            }
                            self.cancel_migration_preparation(format!("{error:#}"));
                        }
                    },
                    Err(error) => {
                        self.bus
                            .senders
                            .send_to_pty(PtyInstruction::SessionMigration(PtyRequest::Resume))
                            .context("resume untouched source session")
                            .non_fatal();
                        self.resume_migration_plugins(&state.operation);
                        self.bus
                            .senders
                            .session_update_quiesced
                            .store(false, std::sync::atomic::Ordering::Release);
                        if let Some(reply) = state.release_reply {
                            let _ = reply.send(Err(format!("{error:#}")));
                        }
                    },
                }
            },
            ScreenRequest::Released { operation, results } => {
                let Some(mut state) = self.migration.take() else {
                    return;
                };
                if state.operation != operation || state.stage != Stage::Releasing {
                    self.migration = Some(state);
                    return;
                }
                // PTY durably records each transition before reporting the batch.
                // Retain those receipts rather than overwriting them with the
                // screen's older pre-release snapshot.
                if let Ok(document) = migration::load_snapshot(&state.path, &operation) {
                    state.snapshot = document;
                }
                state.snapshot.errors = results
                    .into_iter()
                    .filter_map(|(id, result)| {
                        result.err().map(|error| format!("terminal_{id}: {error}"))
                    })
                    .collect();
                if state.snapshot.errors.is_empty() {
                    state.stage = Stage::Released;
                    state.snapshot.phase = MigrationPhase::Released;
                    // Keep the migration guard installed while closing the last
                    // source tab; settle_tabs must not retire this server.
                    let prepared = state.prepared.clone();
                    self.migration = Some(state);
                    self.remove_migration_tabs(&prepared);
                    let Some(retained) = self.migration.take() else {
                        return;
                    };
                    state = retained;
                } else {
                    state.stage = Stage::Recovery;
                    state.snapshot.phase = MigrationPhase::Recovery;
                }
                let saved = migration::save_snapshot(&state.path, &state.snapshot);
                if let Some(reply) = state.release_reply.take() {
                    let result=saved.and_then(|()| {
                        anyhow::ensure!(state.stage==Stage::Released,"some terminal releases are uncertain; retain both servers and recover operation {} from {}",state.operation,state.path.display());
                        Ok(state.snapshot.clone())
                    }).map_err(|e|format!("{e:#}"));
                    let _ = reply.send(result);
                }
                self.migration = Some(state);
            },
            ScreenRequest::Adopt {
                operation,
                snapshot,
                client,
                reply,
            } => {
                let result: Result<()> = (|| {
                    let mut document = migration::load_snapshot(&snapshot, &operation)?;
                    if let Some(state) = self.migration.as_ref()
                        && state.operation == operation
                        && state.stage == Stage::Recovery
                        && state.prepared.is_empty()
                    {
                        // A destination or cold rollback can fail after partial
                        // adoption. Hand back this process's ownership before retrying
                        // from its original grid and durable detached output.
                        let (aborted, received) = crossbeam::channel::bounded(1);
                        self.bus
                            .senders
                            .send_to_pty(PtyInstruction::SessionMigration(PtyRequest::Abort {
                                operation: operation.clone(),
                                reply: aborted,
                            }))
                            .context("reconcile failed source readoption")?;
                        if let Some(state) = self.migration.as_mut() {
                            state.stage = Stage::Recovering;
                        }
                        let senders = self.bus.senders.clone();
                        let retry_reply = reply.clone();
                        std::thread::spawn(move || {
                            let result = received
                                .recv_timeout(std::time::Duration::from_secs(120))
                                .map_err(|error| {
                                    format!("wait for source readoption handback: {error}")
                                })
                                .and_then(|result| result);
                            match result {
                                Ok(()) => {
                                    if let Err(error) = senders.send_to_screen(
                                        ScreenInstruction::SessionMigration(ScreenRequest::Adopt {
                                            operation,
                                            snapshot,
                                            client,
                                            reply: retry_reply.clone(),
                                        }),
                                    ) {
                                        let _ = retry_reply.send(Err(format!(
                                            "retry source readoption: {error:#}"
                                        )));
                                    }
                                },
                                Err(error) => {
                                    let _ = retry_reply.send(Err(error));
                                },
                            }
                        });
                        return Ok(());
                    }
                    if let Some(state) = self.migration.as_mut()
                        && state.operation == operation
                        && matches!(state.stage, Stage::Releasing | Stage::Recovery)
                        && !state.prepared.is_empty()
                        && document.inventory.instance_id == self.instance_id.to_string()
                    {
                        state.stage = Stage::Recovering;
                        if let Some(release) = state.release_reply.take() {
                            let _ = release
                                .send(Err("source recovery superseded the release request".into()));
                        }
                        self.bus
                            .senders
                            .send_to_pty(PtyInstruction::SessionMigration(
                                PtyRequest::ReconcileSource {
                                    operation,
                                    path: snapshot,
                                    client,
                                    reply: reply.clone(),
                                },
                            ))
                            .context("reconcile partially released source ownership")?;
                        return Ok(());
                    }
                    if let Some(state) = &self.migration {
                        anyhow::ensure!(
                            state.operation == operation,
                            "another migration is active"
                        );
                        if state.stage == Stage::Adopted {
                            let _ = reply.send(Ok(state.snapshot.mapping.clone()));
                            return Ok(());
                        }
                        anyhow::ensure!(
                            state.stage == Stage::Released,
                            "migration ownership is not ready for adoption"
                        );
                    }
                    anyhow::ensure!(
                        matches!(
                            document.phase,
                            MigrationPhase::Released | MigrationPhase::Recovery
                        ),
                        "snapshot is not released and adoptable"
                    );
                    anyhow::ensure!(
                        document.errors.is_empty(),
                        "snapshot has unresolved ownership errors; recover those before adoption"
                    );
                    anyhow::ensure!(self.tabs.values().all(|tab|tab.get_all_pane_ids().iter().all(|id|!matches!(id,PaneId::Terminal(_)))),"replacement session already contains live or held terminals; refusing to overwrite them");
                    for tab in &document.tabs {
                        tab.payload
                            .validate_relationships()
                            .map_err(|e| anyhow!("{e}"))?;
                    }
                    for run in &document.inventory.paneless_plugins {
                        self.bus
                            .senders
                            .send_to_plugin(
                                yinz_bus::plugins::PluginInstruction::LoadBackgroundPlugin(
                                    run.clone(),
                                    client,
                                ),
                            )
                            .context("recreate migrated paneless plugin")?;
                    }
                    // Host placement works while guest executors are paused.
                    // Keep rollback plugins paused too: their queued unloads
                    // precede replacement loads, which start only after Finish.
                    document.phase = MigrationPhase::Adopting;
                    migration::save_snapshot(&snapshot, &document)?;
                    self.bus
                        .senders
                        .session_update_quiesced
                        .store(true, std::sync::atomic::Ordering::Release);
                    self.migration = Some(MigrationState {
                        operation,
                        path: snapshot.clone(),
                        stage: Stage::Adopting,
                        snapshot: document.clone(),
                        prepared: vec![],
                        held: HashMap::new(),
                        deadline: Instant::now(),
                        release_reply: None,
                        adoption_reply: Some(reply.clone()),
                        finish_replies: vec![],
                        pending_tabs: HashMap::new(),
                        replay_tabs: Default::default(),
                    });
                    self.bus
                        .senders
                        .send_to_pty(PtyInstruction::SessionMigration(PtyRequest::Adopt {
                            snapshot: Box::new(document),
                            client,
                            path: snapshot,
                            reply: reply.clone(),
                        }))
                        .context("adopt recorded session terminals")
                })();
                if let Err(e) = result {
                    let _ = reply.send(Err(format!("{e:#}")));
                }
            },
            ScreenRequest::Adopted {
                snapshot,
                result,
                client,
                path,
                reply,
            } => {
                let Some(mut state) = self.migration.take() else {
                    let _ = reply.send(Err("destination migration disappeared".into()));
                    return;
                };
                if state.operation != snapshot.operation {
                    self.migration = Some(state);
                    let _ = reply.send(Err("stale session adoption result".into()));
                    return;
                }
                match result {
                    Err(error) => {
                        state.stage = Stage::Recovery;
                        state.snapshot = *snapshot;
                        self.migration = Some(state);
                        let _ = reply.send(Err(error));
                    },
                    Ok(adopted) => {
                        state.snapshot = (*snapshot).clone();
                        let mut error = None;
                        for (tab, panes) in snapshot.tabs.iter().zip(adopted) {
                            let destination = tab.source_tab_id;
                            state.pending_tabs.insert(destination, tab.clone());
                            state.replay_tabs.insert(destination);
                            state
                                .snapshot
                                .mapping
                                .tabs
                                .push((tab.source_tab_id, destination));
                            if let Err(e) = self.place_adopted_tab_at(
                                tab.payload.clone(),
                                panes,
                                client,
                                true,
                                None,
                                None,
                                None,
                                Some(destination),
                            ) {
                                error = Some(e);
                                break;
                            }
                        }
                        state.path = path;
                        self.migration = Some(state);
                        if let Some(error) = error {
                            self.fail_migration_adoption(format!("{error:#}"));
                        }
                    },
                }
            },
            ScreenRequest::SourceReconciled {
                operation,
                path,
                client,
                reply,
                result,
            } => {
                let Some(mut state) = self.migration.take() else {
                    let _ = reply.send(Err(
                        "source migration disappeared during reconciliation".into()
                    ));
                    return;
                };
                if state.operation != operation || state.stage != Stage::Recovering {
                    self.migration = Some(state);
                    let _ = reply.send(Err("stale source reconciliation result".into()));
                    return;
                }
                match result {
                    Err(error) => {
                        state.stage = Stage::Recovery;
                        self.migration = Some(state);
                        let _ = reply.send(Err(error));
                    },
                    Ok(snapshot) => {
                        state.snapshot = *snapshot;
                        state.stage = Stage::Released;
                        let prepared = state.prepared.clone();
                        self.migration = Some(state);
                        self.remove_migration_tabs(&prepared);
                        self.session_migration(ScreenRequest::Adopt {
                            operation,
                            snapshot: path,
                            client,
                            reply,
                        });
                    },
                }
            },
            ScreenRequest::Abort { operation, reply } => match self.migration.as_ref() {
                None => {
                    let _ = reply.send(Ok(()));
                },
                Some(state) if state.operation != operation => {
                    let _ = reply.send(Err("a different session migration is active".into()));
                },
                Some(state) if state.stage == Stage::Released && state.prepared.is_empty() => {
                    // A lost handback acknowledgement must be retryable. Source
                    // releases retain their prepared tabs and still require adoption.
                    // The source may already have updated this shared snapshot;
                    // a repeated destination acknowledgement must not rewrite it.
                    let _ = reply.send(Ok(()));
                },
                Some(state) if state.stage == Stage::Pausing => {
                    self.migration = None;
                    self.bus
                        .senders
                        .send_to_pty(PtyInstruction::SessionMigration(PtyRequest::Resume))
                        .context("resume cancelled source preparation")
                        .non_fatal();
                    self.resume_migration_plugins(&operation);
                    self.bus
                        .senders
                        .session_update_quiesced
                        .store(false, std::sync::atomic::Ordering::Release);
                    let _ = reply.send(Ok(()));
                },
                Some(state)
                    if matches!(
                        state.stage,
                        Stage::Adopting | Stage::Adopted | Stage::Recovery
                    ) && state.prepared.is_empty() =>
                {
                    let sent = self
                        .bus
                        .senders
                        .send_to_pty(PtyInstruction::SessionMigration(PtyRequest::Abort {
                            operation,
                            reply: reply.clone(),
                        }))
                        .context("release destination before source recovery");
                    match sent {
                        Ok(()) => {
                            if let Some(state) = self.migration.as_mut() {
                                state.stage = Stage::Recovering;
                            }
                        },
                        Err(error) => {
                            let _ = reply.send(Err(format!("{error:#}")));
                        },
                    }
                },
                Some(_) => {
                    let _=reply.send(Err("source terminal ownership has changed or is changing; restore the durable snapshot before resuming".into()));
                },
            },
            ScreenRequest::Aborted {
                operation,
                result,
                reply,
            } => {
                let Some(mut state) = self.migration.take() else {
                    let _ =
                        reply.send(Err("destination migration disappeared during abort".into()));
                    return;
                };
                if state.operation != operation {
                    self.migration = Some(state);
                    let _ = reply.send(Err("stale migration abort".into()));
                    return;
                }
                let result = result.and_then(|()| {
                    // The PTY thread acknowledged every handback before any
                    // source can re-adopt. Remove only the destination's pane
                    // objects; ClosePane on a live terminal would kill it.
                    let ids: Vec<_> = self.tabs.keys().copied().collect();
                    for id in ids {
                        if let Some(tab) = self.tabs.get_mut(&id) {
                            let pane_ids = tab.get_all_pane_ids();
                            let _ = tab.extract_warp_panes(&pane_ids);
                        }
                        self.tabs.remove(&id);
                        self.pending_warp_relationships.remove(&id);
                    }
                    self.active_tab_ids.clear();
                    state.pending_tabs.clear();
                    state.snapshot.phase = MigrationPhase::Released;
                    state.snapshot.errors.clear();
                    state.snapshot.mapping = Default::default();
                    for (_, owner) in &mut state.snapshot.ownership {
                        *owner = MigrationOwnership::Released;
                    }
                    migration::save_snapshot(&state.path, &state.snapshot)
                        .map_err(|e| format!("{e:#}"))?;
                    state.stage = Stage::Released;
                    Ok(())
                });
                // Remain quiescent: delayed plugin/layout replies must not
                // re-create panes after they have been given back.
                if result.is_err() {
                    state.stage = Stage::Recovery;
                }
                self.migration = Some(state);
                let _ = reply.send(result);
            },
            ScreenRequest::TabReplayed { operation, tab } => {
                if let Some(state) = self.migration.as_mut()
                    && state.operation == operation
                    && state.stage == Stage::Adopting
                {
                    state.replay_tabs.remove(&tab);
                }
                self.finish_migration_replay();
            },
            ScreenRequest::Rename {
                operation,
                name,
                client,
                reply,
            } => {
                let result = (|| -> Result<()> {
                    let state = self.migration.as_ref().context("no migration is active")?;
                    anyhow::ensure!(
                        state.operation == operation
                            && matches!(state.stage, Stage::Released | Stage::Adopted),
                        "migration is not ready to rename its endpoint"
                    );
                    if self.session_name == name {
                        let ownership = self
                            .bus
                            .senders
                            .session_ownership
                            .as_ref()
                            .and_then(std::sync::Weak::upgrade)
                            .context("missing migration session ownership")?;
                        let destination = yinz_runtime::consts::YINZ_SOCK_DIR.join(&name);
                        if ownership.socket_path() != destination {
                            ownership
                                .relocate(&destination)
                                .context("publish migrated session endpoint")?;
                            self.save_session(None)
                                .context("save published migration checkpoint")?;
                        }
                    } else {
                        // Migration moves an endpoint while transferred
                        // supervisors retain the original logical session.
                        // Relabeling or enumerating detached panes here would
                        // alter ownership outside this migration snapshot.
                        self.rename_session_with_pty_relabel(name.clone(), client, false)
                            .context("rename migration session endpoint")?;
                    }
                    anyhow::ensure!(
                        self.session_name == name,
                        "session endpoint rename was refused"
                    );
                    Ok(())
                })()
                .map_err(|e| format!("{e:#}"));
                let _ = reply.send(result);
            },
            ScreenRequest::Finish { operation, reply } => {
                if self
                    .completed_migration
                    .as_ref()
                    .is_some_and(|(completed, _, _)| completed == &operation)
                {
                    let _ = reply.send(Ok(()));
                    return;
                }
                let result = (|| -> Result<()> {
                    let state = self.migration.as_mut().context("no migration is active")?;
                    anyhow::ensure!(
                        state.operation == operation,
                        "stale session migration finish"
                    );
                    if state.stage == Stage::Finishing {
                        state.finish_replies.push(reply.clone());
                        return Ok(());
                    }
                    anyhow::ensure!(
                        matches!(state.stage, Stage::Adopted | Stage::Repairing),
                        "session migration is not ready to commit"
                    );
                    state.stage = Stage::Finishing;
                    state.finish_replies.push(reply.clone());
                    // Paused background services must see the complete restored
                    // manifest before running again. Intermediate removal or
                    // placement reports could otherwise delete durable latches.
                    if let Err(error) = self.log_and_report_session_state() {
                        self.session_migration(ScreenRequest::PluginsResumed {
                            operation: operation.clone(),
                            result: Err(format!(
                                "publish restored session before plugin resume: {error:#}"
                            )),
                        });
                        return Ok(());
                    }
                    let (tx, rx) = std::sync::mpsc::channel();
                    if let Err(error) = self.bus.senders.send_to_plugin(
                        yinz_bus::plugins::PluginInstruction::ResumeAfterUpdate {
                            operation: operation.clone(),
                            reply: tx,
                        },
                    ) {
                        self.session_migration(ScreenRequest::PluginsResumed {
                            operation: operation.clone(),
                            result: Err(format!("resume migrated plugins: {error:#}")),
                        });
                        return Ok(());
                    }
                    let senders = self.bus.senders.clone();
                    std::thread::spawn(move || {
                        let result = rx
                            .recv_timeout(std::time::Duration::from_secs(65))
                            .map_err(|error| format!("wait for migrated plugin readiness: {error}"))
                            .and_then(|result| result);
                        senders
                            .send_to_screen(ScreenInstruction::SessionMigration(
                                ScreenRequest::PluginsResumed { operation, result },
                            ))
                            .context("report migrated plugin readiness")
                            .non_fatal();
                    });
                    Ok(())
                })();
                if let Err(error) = result {
                    let _ = reply.send(Err(format!("{error:#}")));
                }
            },
            ScreenRequest::PluginsResumed { operation, result } => {
                let Some(state) = self.migration.as_mut() else {
                    return;
                };
                if state.operation != operation || state.stage != Stage::Finishing {
                    return;
                }
                let replies = std::mem::take(&mut state.finish_replies);
                let activation = (|| -> Result<()> {
                    let state = self
                        .migration
                        .as_ref()
                        .context("migration disappeared before commit")?;
                    self.bus
                        .senders
                        .send_to_pty(PtyInstruction::SessionMigration(PtyRequest::Resume))
                        .context("resume migrated terminal output")?;
                    for tab in self.tabs.values_mut() {
                        for id in tab.get_all_pane_ids() {
                            if let Some(pane) = tab.get_pane_with_id_mut(id) {
                                pane.set_warp_pending(false);
                            }
                        }
                    }
                    let map = state.snapshot.mapping.panes.clone();
                    let source =
                        state.snapshot.inventory.instance_id == self.instance_id.to_string();
                    // Process exit notifications may have arrived while readers
                    // were paused. Preserve those, mapping source IDs on rollback;
                    // stale structural callbacks must not target reused IDs.
                    self.migration_deferred.retain_mut(|event| {
                        let id = match event {
                            ScreenInstruction::HoldPane(id, ..)
                            | ScreenInstruction::ClosePane(id, ..) => id,
                            _ => return false,
                        };
                        if source {
                            if let Some((_, to)) = map.iter().find(|(from, _)| from == id) {
                                *id = *to;
                            } else {
                                return false;
                            }
                        }
                        true
                    });
                    self.bus
                        .senders
                        .session_update_quiesced
                        .store(false, std::sync::atomic::Ordering::Release);
                    Ok(())
                })()
                .map_err(|error| format!("{error:#}"));
                let result = match activation {
                    Ok(()) => {
                        if result.is_ok() {
                            if let Some(state) = self.migration.take() {
                                self.completed_migration = Some((
                                    operation.clone(),
                                    state.snapshot.inventory,
                                    state.snapshot.mapping,
                                ));
                            }
                        } else if let Some(state) = self.migration.as_mut() {
                            // The coordinator has durably committed to this
                            // owner before Finish. Keep terminals usable so a
                            // client can repair plugin errors or permissions,
                            // retaining the receipt and backups for Finish retry.
                            state.stage = Stage::Repairing;
                        }
                        result
                    },
                    Err(error) => {
                        if let Some(state) = self.migration.as_mut() {
                            state.stage = Stage::Adopted;
                        }
                        Err(error)
                    },
                };
                for reply in replies {
                    let _ = reply.send(result.clone());
                }
            },
            ScreenRequest::RestoreClient {
                operation,
                source,
                destination,
                reply,
            } => {
                let result = (|| -> Result<()> {
                    let (inventory, mapping) = if let Some(state) = &self.migration {
                        anyhow::ensure!(
                            state.operation == operation
                                && matches!(
                                    state.stage,
                                    Stage::Adopted | Stage::Finishing | Stage::Repairing
                                ),
                            "session migration is not ready to restore clients"
                        );
                        (
                            state.snapshot.inventory.clone(),
                            state.snapshot.mapping.clone(),
                        )
                    } else {
                        let (completed, inventory, mapping) = self
                            .completed_migration
                            .as_ref()
                            .context("no completed migration has client state")?;
                        anyhow::ensure!(
                            completed == &operation,
                            "stale migration client restoration"
                        );
                        (inventory.clone(), mapping.clone())
                    };
                    let focus = inventory
                        .clients
                        .iter()
                        .find(|client| client.client_id == source)
                        .context("source client was not captured by this migration")?;
                    self.restore_migration_client(focus, destination, &mapping)?;
                    self.render().context("render restored migration client")?;
                    self.bus
                        .senders
                        .send_to_server(
                            yinz_bus::server_instruction::ServerInstruction::UpdateClientReady {
                                client: destination,
                                operation: operation.clone(),
                                source_client: source,
                            },
                        )
                        .context("acknowledge restored migration client after rendering")?;
                    Ok(())
                })()
                .map_err(|error| format!("{error:#}"));
                let _ = reply.send(result);
            },
        }
    }

    fn resume_migration_plugins(&self, operation: &str) {
        let (reply, _) = std::sync::mpsc::channel();
        self.bus
            .senders
            .send_to_plugin(yinz_bus::plugins::PluginInstruction::ResumeAfterUpdate {
                operation: operation.into(),
                reply,
            })
            .context("resume plugins after session update")
            .non_fatal();
    }

    fn cancel_migration_preparation(&mut self, error: String) {
        let Some(mut state) = self.migration.take() else {
            return;
        };
        if state.stage != Stage::Pausing {
            self.migration = Some(state);
            return;
        }
        self.bus
            .senders
            .send_to_pty(PtyInstruction::SessionMigration(PtyRequest::Resume))
            .context("resume untouched migration source")
            .non_fatal();
        self.resume_migration_plugins(&state.operation);
        self.bus
            .senders
            .session_update_quiesced
            .store(false, std::sync::atomic::Ordering::Release);
        if let Some(reply) = state.release_reply.take() {
            let _ = reply.send(Err(error));
        }
    }

    pub(super) fn migration_tab_pending(&self, tab: TabId) -> bool {
        self.migration
            .as_ref()
            .is_some_and(|s| s.pending_tabs.contains_key(&tab))
    }

    fn restore_migration_client(
        &mut self,
        focus: &ClientFocus,
        destination: ClientId,
        mapping: &SessionMigrationMap,
    ) -> Result<()> {
        anyhow::ensure!(
            self.connected_clients.borrow().contains(&destination),
            "migration client has not attached"
        );
        let tab = |id: TabId| {
            mapping
                .tabs
                .iter()
                .find(|(from, _)| *from == id)
                .map(|(_, to)| *to)
        };
        let pane = |id: PaneId| {
            mapping
                .panes
                .iter()
                .find(|(from, _)| *from == id)
                .map(|(_, to)| *to)
        };
        let mut mode = focus.mode_info.clone();
        mode.session_name = Some(self.session_name.clone());
        self.mode_info.insert(destination, mode.clone());
        for tab in self.tabs.values_mut() {
            tab.change_mode_info(mode.clone(), destination);
        }
        // A late reattach can follow a natural process exit or a plugin repair
        // that replaced its pane. Keep the valid focus chosen by AddClient
        // when the original target has since disappeared.
        if let Some(tab_id) = focus.tab_id.and_then(tab)
            && let Some(position) = self.tabs.get(&tab_id).map(|tab| tab.position)
        {
            self.switch_active_tab(position, None, true, destination)
                .context("restore migrated client tab")?;
            if let Some(pane_id) = focus.pane_id.and_then(pane)
                && self
                    .tabs
                    .get(&tab_id)
                    .is_some_and(|tab| tab.get_pane_with_id(pane_id).is_some())
            {
                self.tabs
                    .get_mut(&tab_id)
                    .context("missing migrated focus tab")?
                    .focus_pane_with_id(pane_id, false, false, destination)
                    .context("restore migrated client pane")?;
            }
        }
        self.tab_history.insert(
            destination,
            focus
                .tab_history
                .iter()
                .filter_map(|id| tab(*id))
                .filter(|id| self.tabs.contains_key(id))
                .collect(),
        );
        self.pane_history.insert(
            destination,
            focus
                .pane_history
                .iter()
                .filter_map(|id| pane(*id))
                .filter(|id| {
                    self.tabs
                        .values()
                        .any(|tab| tab.get_pane_with_id(*id).is_some())
                })
                .collect(),
        );
        self.current_pane_group.borrow_mut().restore_client_group(
            destination,
            focus
                .marked_panes
                .iter()
                .filter_map(|id| pane(*id))
                .filter(|id| {
                    self.tabs
                        .values()
                        .any(|tab| tab.get_pane_with_id(*id).is_some())
                })
                .collect(),
        );
        self.broadcast_mode_update(mode, Some(destination))
            .context("restore migrated mode presentation")?;
        Ok(())
    }

    pub(super) fn fail_migration_adoption(&mut self, error: String) {
        if let Some(state) = self.migration.as_mut() {
            state.stage = Stage::Recovery;
            state.snapshot.phase = MigrationPhase::Recovery;
            state.snapshot.errors.push(error.clone());
            migration::save_snapshot(&state.path, &state.snapshot)
                .context("record failed session placement")
                .non_fatal();
            if let Some(reply) = state.adoption_reply.take() {
                let _ = reply.send(Err(error));
            }
        }
    }

    fn finish_migration_replay(&mut self) {
        let Some(mut state) = self.migration.take() else {
            return;
        };
        if state.stage != Stage::Adopting
            || !state.pending_tabs.is_empty()
            || !state.replay_tabs.is_empty()
        {
            self.migration = Some(state);
            return;
        }
        let result: Result<()> = (|| {
            self.restore_migration_focus(&state.snapshot.inventory, &state.snapshot.mapping)?;
            state.stage = Stage::Adopted;
            state.snapshot.phase = MigrationPhase::Adopted;
            migration::save_snapshot(&state.path, &state.snapshot)?;
            Ok(())
        })();
        self.migration = Some(state);
        match result {
            Err(error) => self.fail_migration_adoption(format!("{error:#}")),
            Ok(()) => {
                if let Some(state) = self.migration.as_mut()
                    && let Some(reply) = state.adoption_reply.take()
                {
                    let _ = reply.send(Ok(state.snapshot.mapping.clone()));
                }
            },
        }
    }

    pub(super) fn restore_migration_focus(
        &mut self,
        inventory: &SessionMigrationInventory,
        mapping: &SessionMigrationMap,
    ) -> Result<()> {
        let restoring_source = inventory.instance_id == self.instance_id.to_string();
        for focus in &inventory.clients {
            // Numeric client IDs belong to a server incarnation. A replacement
            // bootstrap connection can happen to reuse an original user's ID.
            if restoring_source && self.connected_clients.borrow().contains(&focus.client_id) {
                self.restore_migration_client(focus, focus.client_id, mapping)?;
            }
        }
        let mapped_tab = |id: TabId| {
            mapping
                .tabs
                .iter()
                .find(|(from, _)| *from == id)
                .map(|(_, to)| *to)
        };
        if let Some(tab) = mapped_tab(inventory.global_last_active_tab) {
            // Placing tabs focuses the bootstrap coordinator on each new tab.
            // Its eventual disconnect must not replace the restored default
            // with whichever tab happened to be placed last.
            let bootstrap_clients: Vec<_> = self
                .connected_clients
                .borrow()
                .iter()
                .copied()
                .filter(|id| {
                    !restoring_source
                        || !inventory.clients.iter().any(|focus| focus.client_id == *id)
                })
                .collect();
            let position = self
                .tabs
                .get(&tab)
                .context("missing migrated default tab")?
                .position;
            for client in bootstrap_clients {
                self.switch_active_tab(position, None, true, client)
                    .context("restore migration bootstrap focus")?;
            }
            self.global_last_active_tab_id = tab;
        }
        for (id, preferred) in &inventory.tab_group_preferences {
            if let Some(group) = self.tab_groups.get_mut(id) {
                group.preferred = preferred.and_then(mapped_tab);
            }
        }
        Ok(())
    }

    pub(super) fn finish_migration_tab(&mut self, tab_id: TabId, slots: &[PaneId]) {
        let Some(mut state) = self.migration.take() else {
            return;
        };
        let Some(source) = state.pending_tabs.remove(&tab_id) else {
            self.migration = Some(state);
            return;
        };
        let result: Result<()> = (|| {
            anyhow::ensure!(
                source.slots.len() == slots.len(),
                "migrated layout slot count changed"
            );
            for (source_id, destination_id) in source.slots.iter().zip(slots) {
                state
                    .snapshot
                    .mapping
                    .panes
                    .push((*source_id, *destination_id));
                if let Some((_, saved)) = source
                    .terminal_states
                    .iter()
                    .find(|(id, _)| id == source_id)
                {
                    self.tabs
                        .get_mut(&tab_id)
                        .and_then(|t| t.get_pane_with_id_mut(*destination_id))
                        .context("adopted terminal was not placed")?
                        .restore_migration_state(saved.clone())
                        .map_err(|e| anyhow!("restore {source_id:?}: {e}"))?;
                }
            }
            self.tabs
                .get_mut(&tab_id)
                .context("missing migrated tab state")?
                .restore_migration_state(source.state, &state.snapshot.mapping.panes)
                .context("restore tab behavior")?;
            migration::save_snapshot(&state.path, &state.snapshot)?;
            Ok(())
        })();
        self.migration = Some(state);
        if let Err(error) = result {
            self.fail_migration_adoption(format!("{error:#}"));
        }
    }
}

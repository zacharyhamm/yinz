//! PTY instruction dispatch.

use super::layouts::PendingLayoutOverride;
use super::warp::{release_pane_for_warp, release_tab_with_held_for_warp, strand_pane_after_warp};
use super::{
    finish_command_rerun, pane_start_failure, send_pane_error_to_screen, PanePlacement,
    PendingPaneSpawn, Pty,
};
use crate::background_jobs::BackgroundJob;
use crate::plugins::PluginInstruction;
use crate::pty_host::client::ReleasedPane;
use crate::screen::ScreenInstruction;
use crate::ServerInstruction;
use yinz_bus::pty::{PendingSpawn, PtyInstruction};
use yinz_bus::warp::ReleasedPaneForWarp;
use yinz_runtime::errors::prelude::*;
use yinz_session::session_serialization;
use yinz_types::diagnostics::ContextType;
use yinz_types::input::command::{OpenFilePayload, RunCommand, TerminalAction};
use yinz_types::input::layout::{Layout, Run};
use yinz_types::pane_id::PaneId;

pub fn pty_thread_main(mut pty: Pty, layout: Box<Layout>) -> Result<()> {
    pty.register_child_state_callback();
    pty.register_agent_report_callback();
    loop {
        let (event, mut err_ctx) = pty.bus.recv().expect("failed to receive event on channel");
        yinz_runtime::errors::add_call(&mut err_ctx, ContextType::Pty((&event).into()));
        match event {
            PtyInstruction::SpawnTerminal(
                terminal_action,
                name,
                new_pane_placement,
                start_suppressed,
                client_or_tab_index,
                completion_tx,
                set_blocking,
            ) => {
                let (run_command, pane_title, open_file_payload) = match &terminal_action {
                    Some(TerminalAction::RunCommand(run_command)) => {
                        (Some(run_command.clone()), name, None)
                    },
                    Some(TerminalAction::OpenFile(open_file_payload)) => {
                        (None, name, Some(open_file_payload.clone()))
                    },
                    _ => (None, name, None),
                };
                let invoked_with = match &terminal_action {
                    Some(TerminalAction::RunCommand(run_command)) => {
                        Some(Run::Command(run_command.clone()))
                    },
                    Some(TerminalAction::OpenFile(payload)) => Some(Run::EditFile(
                        payload.path.clone(),
                        payload.line_number,
                        payload.cwd.clone(),
                    )),
                    _ => None,
                };
                pty.spawn_pane(
                    terminal_action,
                    PendingPaneSpawn {
                        run_command,
                        open_file_payload,
                        pane_title,
                        invoked_with,
                        client_or_tab_index,
                        completion_tx,
                        placement: PanePlacement::New {
                            new_pane_placement,
                            start_suppressed,
                            set_blocking,
                        },
                    },
                )?;
            },
            PtyInstruction::SpawnResolved(resolved) => match resolved {
                PendingSpawn::Terminal(pending, spawned) => {
                    pty.handle_remote_spawned(pending, spawned)?;
                },
                PendingSpawn::Rerun(pending, spawned) => {
                    pty.handle_rerun_spawned(*pending, spawned)?;
                },
            },
            PtyInstruction::SpawnInPlaceTerminal(
                terminal_action,
                name,
                replaced,
                client_id_tab_index_or_pane_id,
                completion_tx,
            ) => {
                let (run_command, pane_title, open_file_payload) = match &terminal_action {
                    Some(TerminalAction::RunCommand(run_command)) => {
                        (Some(run_command.clone()), name, None)
                    },
                    Some(TerminalAction::OpenFile(open_file_payload)) => {
                        (None, name, Some(open_file_payload.clone()))
                    },
                    _ => (None, name, None),
                };
                let invoked_with = match &terminal_action {
                    Some(TerminalAction::RunCommand(run_command)) => {
                        Some(Run::Command(run_command.clone()))
                    },
                    Some(TerminalAction::OpenFile(payload)) => Some(Run::EditFile(
                        payload.path.clone(),
                        payload.line_number,
                        payload.cwd.clone(),
                    )),
                    _ => None,
                };
                pty.spawn_pane(
                    terminal_action,
                    PendingPaneSpawn {
                        run_command,
                        open_file_payload,
                        pane_title,
                        invoked_with,
                        client_or_tab_index: client_id_tab_index_or_pane_id,
                        completion_tx,
                        placement: PanePlacement::InPlace { replaced },
                    },
                )?;
            },
            PtyInstruction::OpenInPlaceEditor(
                temp_file,
                line_number,
                client_tab_index_or_pane_id,
                _completion_tx,
            ) => {
                let err_context = || "failed to open in-place editor for client".to_string();

                match pty.spawn_terminal(
                    Some(TerminalAction::OpenFile(OpenFilePayload::new(
                        temp_file,
                        line_number,
                        None,
                    ))),
                    client_tab_index_or_pane_id,
                ) {
                    Ok((pid, _starts_held)) => {
                        pty.bus
                            .senders
                            .send_to_screen(ScreenInstruction::OpenInPlaceEditor(
                                PaneId::Terminal(pid),
                                client_tab_index_or_pane_id,
                            ))
                            .with_context(err_context)?;
                    },
                    Err(e) => {
                        Err::<(), _>(e).with_context(err_context).non_fatal();
                    },
                }
            },
            PtyInstruction::UpdateActivePane(pane_id, client_id) => {
                pty.set_active_pane(pane_id, client_id);
            },
            PtyInstruction::RemoveClient(client_id) => {
                pty.remove_client(client_id);
            },
            PtyInstruction::GoToTab(tab_index, client_id) => {
                pty.bus
                    .senders
                    .send_to_screen(ScreenInstruction::GoToTab(tab_index, Some(client_id), None))
                    .with_context(|| {
                        format!("failed to move client {} to tab {}", client_id, tab_index)
                    })?;
            },
            PtyInstruction::NewTab(
                cwd,
                terminal_action,
                tab_layout,
                floating_panes_layout,
                tab_index,
                plugin_ids,
                initial_panes,
                block_on_first_terminal,
                should_change_focus_to_new_tab,
                client_id,
                completion_tx,
            ) => {
                let err_context = || "failed to open new tab";
                log::info!(
                    "PtyInstruction::NewTab: spawning terminals for tab {}",
                    tab_index
                );

                pty.spawn_terminals_for_layout(
                    cwd,
                    tab_layout.unwrap_or_else(|| layout.new_tab().0),
                    floating_panes_layout,
                    terminal_action.clone(),
                    plugin_ids,
                    initial_panes,
                    tab_index,
                    block_on_first_terminal,
                    should_change_focus_to_new_tab,
                    client_id,
                    completion_tx,
                )
                .with_context(err_context)
                .non_fatal();
            },
            PtyInstruction::OverrideLayout(
                cwd,
                default_shell,
                tabs_to_close,
                tab_layouts_with_plugin_ids,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                client_id,
                completion_tx,
            ) => {
                let pending = PendingLayoutOverride {
                    cwd,
                    default_shell,
                    tabs_to_close,
                    tabs: tab_layouts_with_plugin_ids,
                    retain_existing_terminal_panes,
                    retain_existing_plugin_panes,
                    client_id,
                    completion_tx,
                };
                pty.finish_layout_override(pending)?;
            },
            PtyInstruction::ClosePane(id, _completion_tx) => {
                pty.close_pane(id)
                    .and_then(|_| {
                        pty.bus
                            .senders
                            .send_to_server(ServerInstruction::UnblockInputThread)
                    })
                    .with_context(|| format!("failed to close pane {:?}", id))?;
            },
            PtyInstruction::CloseTab(ids) => {
                pty.close_tab(ids)
                    .and_then(|_| {
                        pty.bus
                            .senders
                            .send_to_server(ServerInstruction::UnblockInputThread)
                    })
                    .context("failed to close tabs")?;
            },
            PtyInstruction::ReleaseTerminalForWarp {
                held,
                terminal_id,
                to_session,
                placement,
                client_id,
                deadline,
            } => {
                let released = if let Some(held) = held {
                    pty.drain_held_reader_for_warp(terminal_id, deadline)
                        .map(|_| ReleasedPaneForWarp {
                            run_command: held.command.clone(),
                            pane: ReleasedPane::Held(held),
                        })
                        .map_err(|e| format!("{e:#}"))
                } else {
                    release_pane_for_warp(&mut pty, terminal_id, &to_session, deadline)
                };
                // Screen hears about this either way: it has a pane sitting in
                // limbo and has to either send it on or put it back.
                let _ = pty
                    .bus
                    .senders
                    .send_to_screen(ScreenInstruction::PaneReleasedForWarp {
                        terminal_id,
                        to_session,
                        placement,
                        client_id,
                        released,
                    });
            },
            PtyInstruction::AdoptTerminal {
                mut payload,
                placement,
                client_id,
                response_channel,
                hold,
                placed,
            } => {
                if let Some(group) = payload.group.take() {
                    let result = group
                        .validate_pane_group()
                        .map_err(|e| anyhow!("{e}"))
                        .and_then(|_| pty.adopt_tab(&group))
                        .and_then(|adopted| {
                            pty.bus
                                .senders
                                .send_to_screen(ScreenInstruction::AdoptedTabReady {
                                    payload: group,
                                    is_rollback: false,
                                    adopted: Box::new(adopted),
                                    client_id,
                                    placement: Some(placement),
                                    response_channel: Some(response_channel.clone()),
                                    hold,
                                })
                                .context("failed to place adopted pane group")
                        });
                    if let Err(error) = result {
                        let _ = response_channel.send(Err(error));
                    }
                    continue;
                }
                // Answered as soon as the supervisor is attached: this session owns
                // it from then on, so the caller must not roll back. Screen checks
                // placement in `AdoptedPaneFinished`, answers `placed` and releases
                // `hold`. The payload, which carries the pane's whole scrollback,
                // moves into the Screen instruction rather than being cloned.
                let held = payload.held.is_some();
                let result = pty.adopt_terminal(&payload).and_then(|adopted| {
                    let terminal_id = adopted.terminal_id;
                    pty.bus
                        .senders
                        .send_to_screen(ScreenInstruction::AdoptedPaneReady {
                            hold,
                            placed,
                            terminal_id,
                            payload,
                            placement,
                            client_id,
                            cmd: adopted.cmd,
                            buffered: adopted.buffered,
                            child_exit_status: adopted.child_exit_status,
                        })
                        .context("failed to place adopted pane")
                        .inspect_err(|_| {
                            // Nothing will place it, so let the caller roll back.
                            pty.give_back_adopted(terminal_id, held);
                        })
                });
                let _ = response_channel.send(result);
            },
            PtyInstruction::ReleaseTerminalsForWarp {
                held,
                tab_index,
                terminal_ids,
                deadline,
            } => {
                let released =
                    release_tab_with_held_for_warp(&mut pty, terminal_ids, held, deadline);
                // Screen hears about this either way: it has a tab full of panes
                // in limbo and has to either send them on or put them back.
                let _ = pty
                    .bus
                    .senders
                    .send_to_screen(ScreenInstruction::TabReleasedForWarp {
                        tab_index,
                        released,
                    });
            },
            PtyInstruction::StrandPaneAfterWarp {
                terminal_id,
                run_command,
                why,
                recovery,
            } => {
                strand_pane_after_warp(&pty, terminal_id, run_command, &why, recovery);
            },
            PtyInstruction::GiveBackAdoptedTerminal {
                terminal_id,
                held,
                recovery,
                placed,
            } => {
                // A live pane that already closed on its own has nothing to give back.
                if held || pty.task_handles.contains_key(&terminal_id) {
                    let error = if recovery.is_empty() {
                        anyhow!("the adopted pane could not be placed, so it was closed")
                    } else {
                        anyhow!(
                            "the adopted pane could not be placed and was given back. \
                             Recover it with `{recovery}`"
                        )
                    };
                    log::error!("adopted pane {terminal_id}: {error}");
                    pty.give_back_adopted(terminal_id, held);
                    // Told only now, so an adopter that rolls back finds the
                    // supervisor already released.
                    if let Some(placed) = placed {
                        let _ = placed.send(Err(error));
                    }
                }
            },
            PtyInstruction::AdoptTab {
                payload,
                client_id,
                is_rollback,
                response_channel,
            } => {
                let result = match pty.adopt_tab(&payload) {
                    Ok(adopted) => {
                        let _ =
                            pty.bus
                                .senders
                                .send_to_screen(ScreenInstruction::AdoptedTabReady {
                                    is_rollback,
                                    placement: None,
                                    response_channel: None,
                                    hold: None,
                                    payload,
                                    adopted: Box::new(adopted),
                                    client_id,
                                });
                        Ok(())
                    },
                    Err(e) => Err(e),
                };
                let _ = response_channel.send(result);
            },
            PtyInstruction::ClaimAdoptedTab {
                tab_index,
                terminals,
            } => {
                pty.adopted_tabs.insert(tab_index, terminals);
            },
            PtyInstruction::ReRunCommandInPane(pane_id, run_command, completion_tx) => {
                if let Some((run_command, _completion_tx)) =
                    pty.request_rerun(pane_id, run_command, completion_tx)?
                {
                    let result = pty.rerun_command_in_pane(pane_id, run_command.clone());
                    finish_command_rerun(&mut pty, pane_id, run_command, result)?;
                }
            },
            PtyInstruction::DropToShellInPane {
                pane_id,
                shell,
                working_dir,
                completion_tx: _completion_tx,
            } => {
                let err_context = || format!("failed to rerun command in pane {:?}", pane_id);

                let run_command = RunCommand {
                    command: shell,
                    hold_on_close: false,
                    hold_on_start: false,
                    cwd: working_dir,
                    ..Default::default()
                };
                // Behind any rerun the pane already has in flight, like every
                // other rerun; a shell is local, so otherwise it runs here.
                let Some((run_command, _)) = pty.request_rerun(pane_id, run_command, None)? else {
                    continue;
                };
                match pty
                    .rerun_command_in_pane(pane_id, run_command.clone())
                    .with_context(err_context)
                {
                    Ok(..) => {},
                    Err(err) => match pane_start_failure(&err) {
                        Some((terminal_id, message)) => {
                            if run_command.hold_on_close {
                                send_pane_error_to_screen(
                                    pty.bus.senders.clone(),
                                    terminal_id,
                                    message,
                                    run_command,
                                )
                                .with_context(err_context)?;
                            }
                        },
                        None => Err::<(), _>(err).non_fatal(),
                    },
                }
            },
            PtyInstruction::DumpLayout(mut session_layout_metadata, client_id, completion_tx) => {
                let err_context = || "Failed to dump layout".to_string();
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                match session_serialization::serialize_session_layout(
                    session_layout_metadata.into(),
                ) {
                    Ok((kdl_layout, _pane_contents)) => {
                        pty.bus
                            .senders
                            .send_to_server(ServerInstruction::Log(
                                vec![kdl_layout],
                                client_id,
                                completion_tx,
                            ))
                            .with_context(err_context)
                            .non_fatal();
                    },
                    Err(e) => {
                        pty.bus
                            .senders
                            .send_to_server(ServerInstruction::Log(
                                vec![e.to_owned()],
                                client_id,
                                completion_tx,
                            ))
                            .with_context(err_context)
                            .non_fatal();
                    },
                }
            },
            PtyInstruction::ListClientsMetadata(
                mut session_layout_metadata,
                client_id,
                completion_tx,
            ) => {
                let err_context = || "Failed to dump layout".to_string();
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                pty.bus
                    .senders
                    .send_to_server(ServerInstruction::Log(
                        vec![session_layout_metadata.list_clients_metadata().to_string()],
                        client_id,
                        completion_tx,
                    ))
                    .with_context(err_context)
                    .non_fatal();
            },
            PtyInstruction::DumpLayoutToPlugin {
                mut session_layout_metadata,
                plugin_id,
                response_channel,
                trace,
            } => {
                trace.mark("pty metadata");
                let err_context = || "Failed to dump layout".to_string();
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                trace.mark("plugin queued");
                pty.bus
                    .senders
                    .send_to_plugin(PluginInstruction::DumpLayoutToPlugin {
                        session_layout_metadata,
                        plugin_id,
                        response_channel,
                        trace,
                    })
                    .with_context(err_context)
                    .non_fatal();
            },
            PtyInstruction::ListClientsToPlugin(
                mut session_layout_metadata,
                plugin_id,
                client_id,
                generation,
            ) => {
                let err_context = || "Failed to dump layout".to_string();
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                pty.bus
                    .senders
                    .send_to_plugin(PluginInstruction::ListClientsToPlugin(
                        session_layout_metadata,
                        plugin_id,
                        client_id,
                        generation,
                    ))
                    .with_context(err_context)
                    .non_fatal();
            },
            PtyInstruction::ReportPluginCwd(plugin_id, cwd) => {
                pty.plugin_cwds.insert(plugin_id, cwd);
            },
            PtyInstruction::LogLayoutToHd(mut session_layout_metadata) => {
                let err_context = || "Failed to dump layout".to_string();
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                if session_layout_metadata.is_dirty() {
                    match session_serialization::serialize_session_layout(
                        session_layout_metadata.into(),
                    ) {
                        Ok(kdl_layout_and_pane_contents) => {
                            pty.bus
                                .senders
                                .send_to_background_jobs(BackgroundJob::ReportLayoutInfo(
                                    kdl_layout_and_pane_contents,
                                ))
                                .with_context(err_context)?;
                        },
                        Err(e) => {
                            log::error!("Failed to log layout to HD: {}", e);
                        },
                    }
                }
            },
            PtyInstruction::SaveSessionToDisk {
                session_name,
                session_info,
                mut session_layout_metadata,
                completion_tx: _completion_tx, // Dropped at end to signal completion
            } => {
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                match session_serialization::serialize_session_layout(
                    session_layout_metadata.into(),
                ) {
                    Ok(kdl_and_files) => {
                        match pty.bus.senders.session_checkpoint.write(
                            session_name,
                            session_info,
                            &kdl_and_files,
                        ) {
                            Ok(()) => {
                                let timestamp_millis = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis()
                                    as u64;
                                let _ = pty.bus.senders.send_to_plugin(
                                    PluginInstruction::UpdateSessionSaveTime(timestamp_millis),
                                );
                            },
                            Err(error) => Err::<(), _>(error).non_fatal(),
                        }

                        let _ = pty.bus.senders.send_to_background_jobs(
                            BackgroundJob::ReportLayoutInfo(kdl_and_files),
                        );
                    },
                    Err(e) => {
                        log::error!("Failed to serialize layout: {}", e);
                    },
                };
            },
            PtyInstruction::ReloadPlugin {
                should_float,
                pane_title,
                run_plugin,
                tab_index,
                size,
                completion_tx,
            } => {
                pty.bus
                    .senders
                    .send_to_plugin(PluginInstruction::Reload(
                        should_float,
                        pane_title,
                        run_plugin,
                        tab_index,
                        size,
                        completion_tx,
                    ))
                    .context("forward plugin reload after queued pane closures")?;
            },
            PtyInstruction::ResolveOverlayTabCwd { id, client_id, cwd } => {
                pty.resolve_overlay_tab_cwd(id, client_id, cwd)?;
            },
            PtyInstruction::FillPluginCwd(
                should_float,
                should_be_open_in_place,
                close_replaced_pane,
                pane_title,
                run,
                tab_index,
                pane_id_to_replace,
                client_id,
                size,
                skip_cache,
                cwd,
                should_focus_plugin,
                floating_pane_coordinates,
                completion_tx,
                overlay_placement,
            ) => {
                pty.fill_plugin_cwd(
                    should_float,
                    should_be_open_in_place,
                    close_replaced_pane,
                    pane_title,
                    run,
                    tab_index,
                    pane_id_to_replace,
                    client_id,
                    size,
                    skip_cache,
                    cwd,
                    should_focus_plugin,
                    floating_pane_coordinates,
                    completion_tx,
                    overlay_placement,
                )?;
            },
            PtyInstruction::Reconfigure {
                default_editor,
                post_command_discovery_hook,
                client_id: _,
            } => {
                pty.reconfigure(default_editor, post_command_discovery_hook);
            },
            PtyInstruction::SendSigintToPaneId(pane_id) => {
                pty.send_sigint_to_pane(pane_id);
            },
            PtyInstruction::SendSigkillToPaneId(pane_id) => {
                pty.send_sigkill_to_pane(pane_id);
            },
            PtyInstruction::ResumeSuspendedPane(pane_id) => {
                pty.resume_suspended_pane(pane_id);
            },
            PtyInstruction::GetPanePid {
                pane_id,
                response_channel,
            } => {
                let response = pty.get_pane_pid(pane_id);
                let _ = response_channel.send(response);
            },
            PtyInstruction::GetPaneRunningCommand {
                pane_id,
                response_channel,
            } => {
                let response = pty.get_pane_running_command(pane_id);
                let _ = response_channel.send(response);
            },
            PtyInstruction::GetPaneCwd {
                pane_id,
                response_channel,
            } => {
                let response = pty.get_pane_cwd(pane_id);
                let _ = response_channel.send(response);
            },
            PtyInstruction::GetPanesMetadata {
                pane_ids,
                response_channel,
            } => {
                let response = pty.get_panes_metadata(&pane_ids);
                let _ = response_channel.send(response);
            },
            PtyInstruction::GetPaneGitProbe {
                refresh_token,
                pane_id,
                cwd,
                response_channel,
            } => {
                let response = pty.get_pane_git_probe(pane_id, &cwd, refresh_token.as_deref());
                let _ = response_channel.send(response);
            },
            PtyInstruction::UpdateAndReportCwds => {
                pty.update_and_report_cwds();
            },
            PtyInstruction::NotifyCwdFromOsc7(terminal_id, path) => {
                pty.notify_cwd_from_osc7(terminal_id, path);
            },
            PtyInstruction::Exit => break,
        }
    }
    Ok(())
}

use super::*;
use yinz_bus::migration::{self, ScreenRequest};
use yinz_types::{internal_ids::SupervisorId, session_migration::MigrationPhase};

fn operation() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn migration_preflight_keeps_all_tabs_and_client_focus_untouched() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(3));
    new_tab(&mut screen, 2, TabId::new(7));
    let before = screen.active_tab_ids.clone();
    let (inventory, _, terminals, held) = screen.migration_inventory(ClientId::new(1)).unwrap();
    assert_eq!(
        inventory
            .tabs
            .iter()
            .map(|tab| tab.tab_id)
            .collect::<Vec<_>>(),
        vec![TabId::new(3), TabId::new(7)]
    );
    assert_eq!(terminals, vec![TerminalId::new(1), TerminalId::new(2)]);
    assert!(held.is_empty());
    assert_eq!(screen.active_tab_ids, before);
    assert!(screen.migration.is_none());
    assert!(!screen
        .bus
        .senders
        .session_update_quiesced
        .load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn migration_preflight_refuses_read_only_watchers_without_mutating_the_session() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(3));
    screen.add_watcher_client(ClientId::new(2)).unwrap();
    let focus = screen.active_tab_ids.clone();
    let error = screen.migration_inventory(ClientId::new(1)).unwrap_err();
    assert!(format!("{error:#}").contains("disconnect read-only watchers"));
    assert_eq!(screen.active_tab_ids, focus);
    assert_eq!(screen.tabs.len(), 1);
    assert!(screen.migration.is_none());
    assert!(!screen
        .bus
        .senders
        .session_update_quiesced
        .load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn migration_suppresses_intermediate_plugin_manifests_until_commit() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(3));
    new_tab(&mut screen, 2, TabId::new(7));
    let (sender, receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    screen.cli_event_subscribers_present = true;
    screen
        .bus
        .senders
        .session_update_quiesced
        .store(true, std::sync::atomic::Ordering::Release);
    // The gate also covers the brief periods when migration's owned state has
    // been taken out of Screen to apply a layout or restore client focus.
    assert!(screen.migration.is_none());
    screen.generate_and_report_pane_state().unwrap();
    screen.generate_and_report_tab_state().unwrap();
    assert!(receiver.try_recv().is_err());
    screen
        .bus
        .senders
        .session_update_quiesced
        .store(false, std::sync::atomic::Ordering::Release);
    screen.generate_and_report_pane_state().unwrap();
    screen.generate_and_report_tab_state().unwrap();
    let updates: Vec<_> = receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates,
            _ => vec![],
        })
        .collect();
    assert!(updates.iter().any(|(_, _, event)| matches!(event, yinz_types::data::Event::PaneUpdate(panes) if panes.panes.len() == 2)));
    assert!(updates.iter().any(
        |(_, _, event)| matches!(event, yinz_types::data::Event::TabUpdate(tabs) if tabs.len() == 2)
    ));
}

#[test]
fn migration_refuses_incomplete_terminal_parser_before_any_release() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"live\x1b[31".to_vec())
        .unwrap();
    let error = screen.migration_inventory(ClientId::new(1)).unwrap_err();
    assert!(format!("{error:#}").contains("escape sequence"));
    assert_eq!(screen.tabs.len(), 1);
    assert!(screen.migration.is_none());
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"mcontinued".to_vec())
        .unwrap();
    screen.migration_inventory(ClientId::new(1)).unwrap();
}

#[test]
fn releasing_the_last_tab_retains_source_server_and_durable_recovery_state() {
    let (mut screen, _, server) =
        create_new_screen_with_capture(Size { cols: 80, rows: 24 }, false, false, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.json");
    let operation = operation();
    let (reply, received) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::Release {
        operation: operation.clone(),
        snapshot: path.clone(),
        client: ClientId::new(1),
        deadline: std::time::Instant::now() + yinz_test_support::default_timeout(),
        reply,
    });
    assert!(screen.migration.is_some());
    assert_eq!(
        migration::load_snapshot(&path, &operation).unwrap().phase,
        MigrationPhase::Prepared
    );
    let address = ReleasedPane::Local(ReleasedPtyHost {
        host_id: SupervisorId::new("test-supervisor"),
        host_pid: ProcessId::new(42),
        control_socket: Some(directory.path().join("supervisor.sock")),
    });
    // The fake has no reader tasks. Drive the barrier acknowledgements directly;
    // the real-OS test verifies those barriers against running supervised children.
    screen.session_migration(ScreenRequest::InputDrained {
        operation: operation.clone(),
        result: Ok(vec![(TerminalId::new(1), address.clone())]),
    });
    assert_eq!(
        migration::load_snapshot(&path, &operation).unwrap().phase,
        MigrationPhase::Releasing
    );
    screen.session_migration(ScreenRequest::Released {
        operation: operation.clone(),
        results: vec![(
            TerminalId::new(1),
            Ok(yinz_bus::warp::ReleasedPaneForWarp {
                pane: address,
                run_command: RunCommand::default(),
            }),
        )],
    });
    let snapshot = received
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.phase, MigrationPhase::Released);
    assert_eq!(snapshot.tabs.len(), 1);
    assert_eq!(snapshot.tabs[0].terminal_states.len(), 1);
    assert!(screen.tabs.is_empty());
    screen.settle_tabs().unwrap();
    assert!(screen.migration.is_some());
    while let Ok((instruction, _)) = server.try_recv() {
        assert!(!matches!(instruction, ServerInstruction::Render(None)));
    }
    assert!(matches!(
        screen.defer_for_migration(ScreenInstruction::Exit),
        Some(ScreenInstruction::Exit)
    ));
}

#[test]
fn cancelling_preparation_releases_the_action_gate_and_rejects_late_barriers() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let directory = tempfile::tempdir().unwrap();
    let operation = operation();
    let (reply, _) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::Release {
        operation: operation.clone(),
        snapshot: directory.path().join("session.json"),
        client: ClientId::new(1),
        deadline: std::time::Instant::now() + yinz_test_support::default_timeout(),
        reply,
    });
    let (reply, result) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::Abort {
        operation: operation.clone(),
        reply,
    });
    result
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
        .unwrap();
    assert!(screen.migration.is_none());
    assert!(!screen
        .bus
        .senders
        .session_update_quiesced
        .load(std::sync::atomic::Ordering::Acquire));
    screen.session_migration(ScreenRequest::PluginsPaused {
        operation,
        result: Ok(()),
    });
    assert!(screen.migration.is_none());
    assert_eq!(screen.tabs.len(), 1);
}

#[test]
fn migrated_client_restores_mode_focus_histories_and_marked_panes_after_commit() {
    use yinz_types::session_migration::SessionMigrationMap;
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(3));
    new_tab(&mut screen, 2, TabId::new(7));
    let source = ClientId::new(1);
    screen
        .tab_history
        .insert(source, vec![TabId::new(3), TabId::new(7)]);
    screen
        .pane_history
        .insert(source, vec![PaneId::Terminal(TerminalId::new(1))]);
    screen
        .change_mode(InputMode::Pane, Some(InputMode::Locked), source)
        .unwrap();
    screen
        .current_pane_group
        .borrow_mut()
        .restore_client_group(source, vec![PaneId::Terminal(TerminalId::new(1))]);
    let (inventory, _, _, _) = screen.migration_inventory(source).unwrap();
    let expected = inventory
        .clients
        .iter()
        .find(|client| client.client_id == source)
        .unwrap()
        .clone();
    let operation = operation();
    let mapping = SessionMigrationMap {
        tabs: vec![
            (TabId::new(3), TabId::new(3)),
            (TabId::new(7), TabId::new(7)),
        ],
        panes: vec![
            (
                PaneId::Terminal(TerminalId::new(1)),
                PaneId::Terminal(TerminalId::new(1)),
            ),
            (
                PaneId::Terminal(TerminalId::new(2)),
                PaneId::Terminal(TerminalId::new(2)),
            ),
        ],
    };
    screen.completed_migration = Some((operation.clone(), inventory, mapping));
    let destination = ClientId::new(2);
    screen.add_client(destination).unwrap();
    let (reply, result) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::RestoreClient {
        operation: operation.clone(),
        source,
        destination,
        reply,
    });
    result
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
        .unwrap();
    assert_eq!(
        screen.active_tab_ids.get(&destination),
        expected.tab_id.as_ref()
    );
    assert_eq!(screen.get_active_pane_id(&destination), expected.pane_id);
    assert_eq!(screen.tab_history[&destination], expected.tab_history);
    assert_eq!(screen.pane_history[&destination], expected.pane_history);
    assert_eq!(screen.mode_info[&destination].mode, InputMode::Pane);
    assert_eq!(
        screen.mode_info[&destination].base_mode,
        Some(InputMode::Locked)
    );
    assert_eq!(
        screen.current_pane_group.borrow().clone_inner()[&destination],
        expected.marked_panes
    );
    screen.close_tab_by_id(expected.tab_id.unwrap()).unwrap();
    let surviving_focus = screen.active_tab_ids[&destination];
    let (reply, restored) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::RestoreClient {
        operation: operation.clone(),
        source,
        destination,
        reply,
    });
    restored.recv().unwrap().unwrap();
    assert_eq!(
        screen.active_tab_ids[&destination], surviving_focus,
        "late clients keep usable focus when their original tab has closed during repair"
    );
    let (reply, result) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::RestoreClient {
        operation: uuid::Uuid::new_v4().to_string(),
        source,
        destination,
        reply,
    });
    assert!(result
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
        .is_err());
}

#[test]
fn migration_bootstrap_disconnect_preserves_default_tab_for_later_attachments() {
    use yinz_types::session_migration::SessionMigrationMap;
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(3));
    new_tab(&mut screen, 2, TabId::new(7));
    let bootstrap = ClientId::new(1);
    assert_eq!(screen.active_tab_ids[&bootstrap], TabId::new(7));
    let (mut inventory, _, _, _) = screen.migration_inventory(bootstrap).unwrap();
    // The replacement coordinator reused the original user's numeric ID, but
    // belongs to a different server incarnation and must not inherit its focus.
    inventory.instance_id = uuid::Uuid::new_v4().to_string();
    inventory.global_last_active_tab = TabId::new(3);
    let mapping = SessionMigrationMap {
        tabs: vec![
            (TabId::new(3), TabId::new(3)),
            (TabId::new(7), TabId::new(7)),
        ],
        panes: vec![],
    };
    screen
        .restore_migration_focus(&inventory, &mapping)
        .unwrap();
    screen.remove_client(bootstrap).unwrap();
    assert_eq!(screen.global_last_active_tab_id, TabId::new(3));
    let attached = ClientId::new(2);
    screen.add_client(attached).unwrap();
    assert_eq!(screen.active_tab_ids[&attached], TabId::new(3));
}

#[test]
fn repeated_destination_abort_does_not_overwrite_later_source_recovery() {
    use yinz_types::session_migration::{SessionMigrationSnapshot, SESSION_MIGRATION_FORMAT};
    let mut source = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut source, 1, TabId::new(0));
    let (inventory, _, _, _) = source.migration_inventory(ClientId::new(1)).unwrap();
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot.json");
    let operation = operation();
    let mut snapshot = SessionMigrationSnapshot {
        format: SESSION_MIGRATION_FORMAT,
        operation: operation.clone(),
        inventory,
        phase: MigrationPhase::Released,
        tabs: vec![],
        mapping: Default::default(),
        errors: vec![],
        ownership: vec![],
    };
    migration::save_snapshot(&path, &snapshot).unwrap();
    let (reply, _) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::Adopt {
        operation: operation.clone(),
        snapshot: path.clone(),
        client: ClientId::new(1),
        reply,
    });
    assert!(screen.migration.is_some());
    let (reply, result) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::Aborted {
        operation: operation.clone(),
        result: Ok(()),
        reply,
    });
    result.recv().unwrap().unwrap();
    // Another owner has since advanced this shared journal. A retried Abort
    // acknowledges the old handback without reverting the new owner's receipt.
    snapshot.phase = MigrationPhase::Adopted;
    migration::save_snapshot(&path, &snapshot).unwrap();
    let expected = std::fs::read(&path).unwrap();
    let (reply, result) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::Abort { operation, reply });
    result.recv().unwrap().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), expected);
}

#[test]
fn migration_serializes_overlapping_abort_requests_and_retries_failed_handback() {
    use yinz_bus::migration::PtyRequest;
    use yinz_types::session_migration::{SessionMigrationSnapshot, SESSION_MIGRATION_FORMAT};
    let mut source = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut source, 1, TabId::new(0));
    let (inventory, _, _, _) = source.migration_inventory(ClientId::new(1)).unwrap();
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot.json");
    let operation = operation();
    let snapshot = SessionMigrationSnapshot {
        format: SESSION_MIGRATION_FORMAT,
        operation: operation.clone(),
        inventory,
        phase: MigrationPhase::Released,
        tabs: vec![],
        mapping: Default::default(),
        errors: vec![],
        ownership: vec![],
    };
    migration::save_snapshot(&path, &snapshot).unwrap();
    let (sender, pty): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
    let (reply, _) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::Adopt {
        operation: operation.clone(),
        snapshot: path.clone(),
        client: ClientId::new(1),
        reply,
    });
    while pty.try_recv().is_ok() {}
    for outcome in [Err("injected ownership handback failure".into()), Ok(())] {
        let (reply, result) = crossbeam::channel::bounded(1);
        screen.session_migration(ScreenRequest::Abort {
            operation: operation.clone(),
            reply,
        });
        let (instruction, _) = pty
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        let PtyInstruction::SessionMigration(PtyRequest::Abort { reply, .. }) = instruction else {
            panic!("expected one ownership handback request");
        };
        let (duplicate, duplicate_result) = crossbeam::channel::bounded(1);
        screen.session_migration(ScreenRequest::Abort {
            operation: operation.clone(),
            reply: duplicate,
        });
        assert!(duplicate_result.recv().unwrap().is_err());
        assert!(
            pty.try_recv().is_err(),
            "a concurrent Abort must not enqueue a second handback callback"
        );
        let succeeds = outcome.is_ok();
        screen.session_migration(ScreenRequest::Aborted {
            operation: operation.clone(),
            result: outcome,
            reply,
        });
        assert_eq!(result.recv().unwrap().is_ok(), succeeds);
    }
    assert_eq!(
        migration::load_snapshot(&path, &operation).unwrap().phase,
        MigrationPhase::Released
    );
}

#[test]
fn migration_plugin_failure_keeps_terminals_interactive_and_finish_retryable() {
    use yinz_bus::migration::PtyRequest;
    use yinz_types::session_migration::{SessionMigrationSnapshot, SESSION_MIGRATION_FORMAT};
    let mut original = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut original, 1, TabId::new(0));
    let (inventory, _, _, _) = original.migration_inventory(ClientId::new(1)).unwrap();
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot.json");
    let operation = operation();
    let snapshot = SessionMigrationSnapshot {
        format: SESSION_MIGRATION_FORMAT,
        operation: operation.clone(),
        inventory,
        phase: MigrationPhase::Released,
        tabs: vec![],
        mapping: Default::default(),
        errors: vec![],
        ownership: vec![],
    };
    migration::save_snapshot(&path, &snapshot).unwrap();
    let (sender, pty): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
    let (sender, plugins): ChannelWithContext<PluginInstruction> = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    let (sender, screen_events): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(sender));
    let (reply, adopted) = crossbeam::channel::bounded(1);
    screen.session_migration(ScreenRequest::Adopt {
        operation: operation.clone(),
        snapshot: path.clone(),
        client: ClientId::new(1),
        reply,
    });
    screen.session_migration(ScreenRequest::TabReplayed {
        operation: operation.clone(),
        tab: TabId::new(0),
    });
    adopted.recv().unwrap().unwrap();
    new_tab(&mut screen, 1, TabId::new(0));
    for succeeds in [false, true] {
        let (reply, finished) = crossbeam::channel::bounded(1);
        screen.session_migration(ScreenRequest::Finish {
            operation: operation.clone(),
            reply,
        });
        let resume = plugins
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::ResumeAfterUpdate { reply, .. } => Some(reply),
                _ => None,
            })
            .expect("Finish must request fresh plugin readiness");
        resume
            .send(if succeeds {
                Ok(())
            } else {
                Err("plugin needs permission".into())
            })
            .unwrap();
        let (ScreenInstruction::SessionMigration(result), _) = screen_events
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
        else {
            panic!("expected plugin readiness result");
        };
        screen.session_migration(result);
        assert_eq!(finished.recv().unwrap().is_ok(), succeeds);
        assert!(!screen.migration_blocks_normal_work());
        assert!(screen
            .defer_for_migration(ScreenInstruction::RequestRepaint(ClientId::new(1)))
            .is_some());
        if !succeeds {
            let (reply, restored) = crossbeam::channel::bounded(1);
            screen.session_migration(ScreenRequest::RestoreClient {
                operation: operation.clone(),
                source: ClientId::new(1),
                destination: ClientId::new(1),
                reply,
            });
            restored.recv().unwrap().unwrap();
            let (reply, aborted) = crossbeam::channel::bounded(1);
            screen.session_migration(ScreenRequest::Abort {
                operation: operation.clone(),
                reply,
            });
            assert!(
                aborted.recv().unwrap().is_err(),
                "fresh input makes ownership rollback unsafe"
            );
            let (reply, readopted) = crossbeam::channel::bounded(1);
            screen.session_migration(ScreenRequest::Adopt {
                operation: operation.clone(),
                snapshot: path.clone(),
                client: ClientId::new(1),
                reply,
            });
            assert!(
                readopted.recv().unwrap().is_err(),
                "fresh input must not be replaced by a stale snapshot"
            );
        }
    }
    assert!(screen.migration.is_none());
    assert!(screen.completed_migration.is_some());
    assert!(pty.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PtyInstruction::SessionMigration(PtyRequest::Resume)
    )));
}

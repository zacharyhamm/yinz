use super::*;

#[test]
fn global_close_cleans_up_once_before_a_failing_reflow() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    let (pty_tx, pty_rx) = channels::unbounded::<(PtyInstruction, ErrorContext)>();
    screen.global_panes = GlobalPaneManager::new(
        ThreadSenders {
            to_pty_writer: Some(SenderWithContext::new(pty_writer_tx)),
            ..Default::default()
        },
        screen.character_cell_size.clone(),
        PaneFrameStyle::None,
    );
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    screen.bus.senders.should_silently_fail = false;
    add_global_terminal(&mut screen, 42);
    // a second pane, on a free edge and allocated after the first, so removing the first regrows it
    // and the reflow has a resize to send (which the dropped receiver below makes fail)
    add_global_terminal_on_edge(&mut screen, 43, zellij_utils::data::GlobalPaneEdge::Left);
    while pty_writer_rx.try_recv().is_ok() {}
    drop(pty_writer_rx);

    assert!(screen
        .close_global_pane(PaneId::Terminal(42), None)
        .is_err());

    assert!(screen.global_panes.pane(PaneId::Terminal(42)).is_none());
    assert_eq!(
        pty_rx
            .try_iter()
            .filter(|(instruction, _)| matches!(
                instruction,
                PtyInstruction::ClosePane(PaneId::Terminal(42), None)
            ))
            .count(),
        1
    );
}

#[test]
fn resurrection_layout_global_panes_launch_once_in_declaration_order_before_tab_panes() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let terminal_placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Bottom,
        size: PercentOrFixed::Fixed(3),
        length: None,
    };
    let plugin_placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Right,
        size: PercentOrFixed::Fixed(8),
        length: None,
    };
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/layout/plugin", &None, None, None).unwrap();
    mock_screen.default_layout.global_panes = vec![
        GlobalPaneLayout {
            run: Some(Run::Command(RunCommand {
                command: PathBuf::from("layout-command"),
                ..Default::default()
            })),
            placement: terminal_placement,
            borderless: true,
            focus: false,
            hidden: true,
        },
        GlobalPaneLayout {
            run: Some(Run::Plugin(run_plugin)),
            placement: plugin_placement,
            borderless: false,
            focus: true,
            hidden: false,
        },
    ];
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    mock_screen.new_tab(TiledPaneLayout::default());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let pty_instructions: Vec<_> = pty_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .filter(|instruction| {
            matches!(
                instruction,
                PtyInstruction::SpawnTerminal(..) | PtyInstruction::FillPluginCwd(..)
            )
        })
        .collect();
    let plugin_instructions: Vec<_> = plugin_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .filter(|instruction| {
            matches!(
                instruction,
                PluginInstruction::Load(..) | PluginInstruction::NewTab(..)
            )
        })
        .collect();
    mock_screen.teardown(vec![screen_thread]);

    assert_eq!(pty_instructions.len(), 1);
    assert!(matches!(
        &pty_instructions[0],
        PtyInstruction::SpawnTerminal(
            Some(TerminalAction::RunCommand(_)),
            _,
            _,
            _,
            _,
            _,
            _,
            Some(properties),
        ) if properties.placement == terminal_placement
            && properties.borderless
            && properties.initially_hidden
            && !properties.should_focus
            && properties.declaration_order == Some(0)
    ));
    assert_eq!(plugin_instructions.len(), 3);
    assert!(matches!(
        &plugin_instructions[0],
        PluginInstruction::Load(
            _, _, _, _, _, _, _, _, _, _, _, _, _, _, _, Some(properties), _
        ) if properties.placement == plugin_placement
            && !properties.borderless
            && !properties.initially_hidden
            && properties.should_focus
            && properties.declaration_order == Some(1)
    ));
    assert!(matches!(
        plugin_instructions[1],
        PluginInstruction::NewTab(..)
    ));
    assert!(matches!(
        plugin_instructions[2],
        PluginInstruction::NewTab(..)
    ));
}

#[test]
fn global_plugin_launch_retains_placement_when_not_loaded() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Right,
        size: PercentOrFixed::Fixed(9),
        length: None,
    };
    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    route_arbitrary_action_to_server(
        &session_metadata,
        Action::LaunchOrFocusPlugin {
            plugin: RunPluginOrAlias::from_url("file:/path/to/global/plugin", &None, None, None)
                .unwrap(),
            should_float: false,
            move_to_focused_tab: false,
            should_open_in_place: false,
            close_replaced_pane: false,
            skip_cache: false,
            tab_id: None,
            global_placement: Some(placement),
        },
        10,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    assert!(received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .any(|instruction| matches!(
            instruction,
            PtyInstruction::FillPluginCwd(
                _, _, _, _, _, _, _, _, _, _, _, _, _, _, Some(actual), _
            ) if actual.placement == placement
        )));
}

#[test]
fn global_hocus_sidebar_restore_reuses_the_hidden_plugin() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/hocus.wasm", &None, None, None).unwrap();
    let global_id = PaneId::Plugin(77);
    add_matching_global_plugin(&mut screen, 77, run_plugin.clone(), false);
    screen
        .focus_pane_with_id(global_id, false, false, 1)
        .unwrap();
    // move the focus back into a tab - switching tabs alone would keep it on the global pane
    screen
        .focus_pane_with_id(PaneId::Terminal(1), false, false, 1)
        .unwrap();
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(1)));
    assert!(screen.global_panes.visible_pane_ids().contains(&global_id));

    screen.hide_pane(global_id).unwrap();
    assert!(screen.global_panes.is_hidden(global_id).unwrap());
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    let mut completion = Some(NotificationEnd::new(completion_tx));

    assert!(screen
        .focus_global_plugin_pane(&run_plugin, 1, &mut completion)
        .unwrap());
    drop(completion);

    assert_eq!(screen.active_tab_ids[&1], 0);
    assert_eq!(screen.global_panes.plugin_ids(), vec![77]);
    assert!(screen.global_panes.visible_pane_ids().contains(&global_id));
    assert_eq!(screen.global_panes.focused_pane_id(1), Some(global_id));
    assert_eq!(
        completion_rx.blocking_recv().unwrap().affected_pane_id,
        Some(global_id)
    );
    assert!(screen.tabs.values().all(|tab| tab
        .pane_infos()
        .iter()
        .all(|pane| !(pane.is_plugin && pane.id == 77))));
    assert!(screen.tabs.values().all(|tab| tab
        .get_floating_panes()
        .all(|(pane_id, _)| *pane_id != global_id)));
    assert!(!plugin_rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PluginInstruction::Load(..))));
}

#[test]
fn global_plugin_launch_focuses_visible_match_without_moving_tabs_or_loading() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin", &None, None, None).unwrap();
    add_matching_global_plugin(&mut screen, 77, run_plugin.clone(), false);
    let active_tab = screen.active_tab_ids[&1];
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    let mut completion = Some(NotificationEnd::new(completion_tx));

    assert!(screen
        .focus_global_plugin_pane(&run_plugin, 1, &mut completion)
        .unwrap());
    drop(completion);

    assert_eq!(screen.active_tab_ids[&1], active_tab);
    assert_eq!(screen.global_panes.plugin_ids(), vec![77]);
    assert_eq!(
        screen.global_panes.focused_pane_id(1),
        Some(PaneId::Plugin(77))
    );
    assert_eq!(
        completion_rx.blocking_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(77))
    );
}

#[test]
fn global_plugin_launch_rejects_a_matching_tab_local_plugin() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/fake/plugin", &None, None, None).unwrap();
    let initial_layout = TiledPaneLayout {
        run: Some(Run::Plugin(run_plugin.clone())),
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();

    mock_screen
        .to_screen
        .send(ScreenInstruction::LaunchOrFocusPlugin(
            run_plugin,
            false,
            false,
            false,
            false,
            None,
            false,
            1,
            Some(NotificationEnd::new(completion_tx)),
            None,
            Some(GlobalPanePlacement {
                edge: GlobalPaneEdge::Right,
                size: PercentOrFixed::Fixed(8),
                length: None,
            }),
        ))
        .unwrap();
    let completion = completion_rx.blocking_recv().unwrap();
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    mock_screen.teardown(vec![screen_thread]);

    assert_eq!(completion.exit_status, Some(1));
    assert!(completion.error_message.unwrap().contains("tab-local pane"));
    assert!(!pty_receiver
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PtyInstruction::FillPluginCwd(..))));
}

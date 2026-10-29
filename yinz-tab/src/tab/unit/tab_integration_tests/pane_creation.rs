use super::*;
use yinz_types::input::layout::Run;
use yinz_types::pane_size::Offset;

#[test]
fn warp_companion_initial_contents_use_parked_geometry() {
    use yinz_types::internal_ids::PluginId;

    let client = ClientId::new(1);
    for id in [
        PaneId::Terminal(TerminalId::new(2)),
        PaneId::Plugin(PluginId::new(2)),
    ] {
        let (mut tab, _plugin_receiver) = create_new_tab_with_plugin_receiver(
            Size {
                cols: 120,
                rows: 40,
            },
            ModeInfo::default(),
        );
        tab.create_warp_companion(
            id,
            FloatingPaneLayout {
                pane_initial_contents: Some("restored companion\r\nsecond row".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(
            tab.get_pane_with_id(id)
                .unwrap()
                .dump_screen(false, Some(client))
                .trim_end(),
            "restored companion\nsecond row",
            "{id:?}"
        );
    }
}

#[test]
fn focused_directional_splits_preserve_launch_command_and_title() {
    for direction in [Direction::Right, Direction::Down] {
        for hold_on_start in [false, true] {
            for title in [None, Some("user title".to_owned())] {
                let mut tab = create_new_tab(
                    Size {
                        cols: 120,
                        rows: 40,
                    },
                    ModeInfo::default(),
                );
                let pane_id = PaneId::Terminal(TerminalId::new(2));
                let command = RunCommand {
                    command: PathBuf::from("sleep"),
                    args: vec!["60".into()],
                    hold_on_start,
                    ..Default::default()
                };
                let invoked_with = Some(Run::Command(command.clone()));
                tab.new_pane(
                    pane_id,
                    title.clone(),
                    invoked_with.clone(),
                    false,
                    true,
                    NewPanePlacement::Tiled {
                        direction: Some(direction),
                        borderless: None,
                    },
                    Some(ClientId::new(1)),
                    None,
                )
                .unwrap();
                if hold_on_start {
                    tab.hold_pane(pane_id, None, true, command.clone());
                }
                let pane = tab.get_active_pane(ClientId::new(1)).unwrap();
                assert_eq!(pane.pid(), pane_id);
                assert_eq!(
                    pane.current_title(),
                    title.clone().unwrap_or_else(|| command.to_string())
                );
                assert_eq!(pane.invoked_with(), &invoked_with);
                assert_eq!(pane.custom_title(), title);
            }
        }
    }
}

#[test]
fn keyboard_horizontal_resize_retries_when_rounding_prevents_movement() {
    let mut tab = create_new_tab_with_layout(
        Size { cols: 12, rows: 20 },
        ModeInfo::default(),
        r#"layout split_direction="vertical" { pane size="53%" focus=true; pane; }"#,
    );
    let left = PaneId::Terminal(TerminalId::new(0));
    let right = PaneId::Terminal(TerminalId::new(1));
    for id in [left, right] {
        assert_eq!(
            tab.get_pane_with_id(id)
                .unwrap()
                .current_geom()
                .cols
                .as_usize(),
            6
        );
    }
    tab.resize(
        ClientId::new(1),
        ResizeStrategy::new(Resize::Decrease, Some(Direction::Right)),
    )
    .unwrap();
    for (id, x, cols) in [(left, 0, 5), (right, 5, 7)] {
        let geom = tab.get_pane_with_id(id).unwrap().current_geom();
        assert_eq!((geom.x, geom.cols.as_usize()), (x, cols));
    }
}

#[test]
fn tiled_resize_only_notifies_panes_whose_content_size_changed() {
    let client = ClientId::new(1);
    let first = PaneId::Terminal(TerminalId::new(1));
    for use_strategies in [false, true] {
        let (sender, receiver) = channels::unbounded();
        let mut tab = create_new_tab_with_mock_pty_writer(
            Size {
                cols: 121,
                rows: 20,
            },
            ModeInfo::default(),
            SenderWithContext::new(sender),
        );
        for id in [2, 3] {
            tab.vertical_split(
                PaneId::Terminal(TerminalId::new(id)),
                None,
                None,
                client,
                None,
                None,
            )
            .unwrap();
        }
        while receiver.try_recv().is_ok() {}
        let strategy = ResizeStrategy::new(Resize::Increase, Some(Direction::Right));
        if use_strategies {
            assert!(tab
                .tiled_panes
                .resize_pane_with_strategies(first, &[strategy], (5.0, 5.0))
                .unwrap());
        } else {
            assert!(tab
                .tiled_panes
                .resize_pane_with_id(strategy, first, None)
                .unwrap());
        }
        let mut resized = Vec::new();
        while let Ok((instruction, _)) = receiver.try_recv() {
            if let PtyWriteInstruction::ResizePty(id, ..) = instruction {
                resized.push(id);
            }
        }
        resized.sort_unstable();
        assert_eq!(resized, vec![TerminalId::new(1), TerminalId::new(2)]);

        assert!(!tab
            .tiled_panes
            .resize_pane_with_strategies(first, &[], (5.0, 5.0))
            .unwrap());
        assert!(
            receiver.try_recv().is_err(),
            "an empty resize must not notify panes"
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Placement {
    NoPreference,
    Tiled,
    Stacked,
}

fn create(
    tab: &mut Tab,
    placement: Placement,
    pid: PaneId,
    suppressed: bool,
    focus: bool,
    borderless: bool,
) -> Result<()> {
    let title = Some("characterized pane".to_owned());
    let run = Some(Run::Command(RunCommand::default()));
    match placement {
        Placement::NoPreference => tab.new_no_preference_pane(
            pid,
            title,
            run,
            suppressed,
            focus,
            Some(crate::ClientId::new(1)),
            None,
            Some(borderless),
        ),
        Placement::Tiled => tab.new_tiled_pane(
            pid,
            title,
            run,
            suppressed,
            focus,
            Some(crate::ClientId::new(1)),
            None,
            Some(borderless),
        ),
        Placement::Stacked => tab.new_stacked_pane(
            pid,
            title,
            run,
            suppressed,
            focus,
            Some(PaneId::Terminal(crate::TerminalId::new(1))),
            Some(crate::ClientId::new(1)),
            None,
            Some(borderless),
        ),
    }
}

#[test]
fn pane_creation_characterization() {
    for placement in [
        Placement::NoPreference,
        Placement::Tiled,
        Placement::Stacked,
    ] {
        for pid in [
            PaneId::Terminal(crate::TerminalId::new(2)),
            PaneId::Plugin(crate::PluginId::new(2)),
        ] {
            for suppressed in [false, true] {
                for focus in [false, true] {
                    for borderless in [false, true] {
                        let (mut tab, plugin_rx) = create_new_tab_with_plugin_receiver(
                            Size {
                                cols: 120,
                                rows: 40,
                            },
                            ModeInfo::default(),
                        );
                        let (tx, pty_rx) = channels::unbounded();
                        if matches!(pid, PaneId::Terminal(_)) {
                            tab = create_new_tab_with_mock_pty_writer(
                                Size {
                                    cols: 120,
                                    rows: 40,
                                },
                                ModeInfo::default(),
                                SenderWithContext::new(tx),
                            );
                        }
                        create(&mut tab, placement, pid, suppressed, focus, borderless).unwrap();
                        assert_eq!(tab.suppressed_panes.contains_key(&pid), suppressed);
                        assert_eq!(tab.tiled_panes.get_pane(pid).is_some(), !suppressed);
                        assert!(!tab.floating_panes.panes_contain(&pid));
                        assert_eq!(
                            tab.get_active_pane_id(crate::ClientId::new(1)),
                            Some(if focus && !suppressed {
                                pid
                            } else {
                                PaneId::Terminal(crate::TerminalId::new(1))
                            })
                        );
                        let pane = if suppressed {
                            &tab.suppressed_panes[&pid].pane
                        } else {
                            tab.tiled_panes.get_pane(pid).unwrap()
                        };
                        assert_eq!(pane.current_title(), "characterized pane");
                        assert_eq!(
                            pane.invoked_with(),
                            &Some(Run::Command(RunCommand::default()))
                        );
                        assert_eq!(pane.borderless(), borderless);
                        if suppressed {
                            assert_eq!(
                                pane.position_and_size(),
                                crate::panes::floating_panes::floating_pane_grid::half_size_middle_geom(
                                    &tab.viewport.borrow(),
                                    0
                                )
                            );
                            assert_eq!(pane.get_content_offset(), Offset::frame(1));
                            assert!(tab.suppressed_panes[&pid].restored_by.is_none());
                        }
                        let cols = pane.get_content_columns();
                        let rows = pane.get_content_rows();
                        let pty_messages: Vec<_> = pty_rx.try_iter().map(|(msg, _)| msg).collect();
                        let plugin_messages: Vec<_> =
                            plugin_rx.try_iter().map(|(msg, _)| msg).collect();
                        // Every placement tells the new pane its size, a named-stack
                        // insertion included: joining a stack used to skip the pty resize
                        // unless the pane was focused and a relayout followed.
                        match pid {
                            PaneId::Terminal(id) => assert!(pty_messages.iter().any(|msg| matches!(*msg, PtyWriteInstruction::ResizePty(p, c, r, None, None) if p == id && usize::from(c) == cols && usize::from(r) == rows)), "placement={placement:?}, {pid:?}, suppressed={suppressed}, focus={focus}, borderless={borderless}, size={cols}x{rows}, messages={pty_messages:?}"),
                            PaneId::Plugin(id) => assert!(plugin_messages.iter().any(|msg| matches!(*msg, PluginInstruction::Resize(p, c, r) if p == id && c == cols && r == rows)), "placement={placement:?}, {pid:?}, suppressed={suppressed}, focus={focus}, borderless={borderless}, size={cols}x{rows}, messages={plugin_messages:?}"),
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn pane_creation_failed_resize_does_not_park() {
    for placement in [
        Placement::NoPreference,
        Placement::Tiled,
        Placement::Stacked,
    ] {
        for pid in [
            PaneId::Terminal(crate::TerminalId::new(2)),
            PaneId::Plugin(crate::PluginId::new(2)),
        ] {
            let (mut tab, plugin_rx) = create_new_tab_with_plugin_receiver(
                Size {
                    cols: 120,
                    rows: 40,
                },
                ModeInfo::default(),
            );
            let (tx, rx) = channels::unbounded();
            tab.senders
                .replace_to_pty_writer(SenderWithContext::new(tx));
            tab.senders.should_silently_fail = false;
            drop(rx);
            drop(plugin_rx);
            let error = create(&mut tab, placement, pid, true, false, true).unwrap_err();
            assert!(format!("{error:#}").contains(&format!("{pid:?}")));
            assert!(!tab.suppressed_panes.contains_key(&pid));
            assert!(tab.tiled_panes.get_pane(pid).is_none());
            assert!(!tab.floating_panes.panes_contain(&pid));
            assert_eq!(
                tab.get_active_pane_id(crate::ClientId::new(1)),
                Some(PaneId::Terminal(crate::TerminalId::new(1)))
            );
        }
    }
}

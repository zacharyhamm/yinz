use super::*;
use yinz_types::input::layout::Run;
use yinz_types::pane_size::Offset;

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
                client,
                None,
                None,
            )
            .unwrap();
        }
        while receiver.try_recv().is_ok() {}
        let strategy = ResizeStrategy::new(Resize::Increase, Some(Direction::Right));
        if use_strategies {
            tab.tiled_panes
                .resize_pane_with_strategies(first, &[strategy], (5.0, 5.0))
                .unwrap();
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

        tab.tiled_panes
            .resize_pane_with_strategies(first, &[], (5.0, 5.0))
            .unwrap();
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
                                crate::tab::half_size_middle_geom(&tab.viewport.borrow(), 0)
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

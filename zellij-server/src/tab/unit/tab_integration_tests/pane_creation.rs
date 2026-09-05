use super::*;
use zellij_utils::input::layout::Run;
use zellij_utils::pane_size::Offset;

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
            Some(1),
            None,
            Some(borderless),
        ),
        Placement::Tiled => tab.new_tiled_pane(
            pid,
            title,
            run,
            suppressed,
            focus,
            Some(1),
            None,
            Some(borderless),
        ),
        Placement::Stacked => tab.new_stacked_pane(
            pid,
            title,
            run,
            suppressed,
            focus,
            Some(PaneId::Terminal(1)),
            Some(1),
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
        for pid in [PaneId::Terminal(2), PaneId::Plugin(2)] {
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
                            tab.get_active_pane_id(1),
                            Some(if focus && !suppressed {
                                pid
                            } else {
                                PaneId::Terminal(1)
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
                        // Named-stack insertion only reapplies frames when focused.
                        if placement == Placement::Stacked && !suppressed && !focus {
                            assert!(!pty_messages
                                .iter()
                                .any(|msg| matches!(msg, PtyWriteInstruction::ResizePty(2, ..))));
                            assert!(!plugin_messages
                                .iter()
                                .any(|msg| matches!(msg, PluginInstruction::Resize(2, ..))));
                            continue;
                        }
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
        for pid in [PaneId::Terminal(2), PaneId::Plugin(2)] {
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
            assert_eq!(tab.get_active_pane_id(1), Some(PaneId::Terminal(1)));
        }
    }
}

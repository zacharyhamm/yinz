use super::*;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

fn mark_agent(tab: &mut Tab, id: PaneId) {
    tab.set_pane_agent(
        id,
        Some(AgentInfo {
            kind: "codex".into(),
            state: AgentState::Busy,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: None,
            run_id: None,
        }),
    );
}

fn draw(tab: &mut Tab) -> String {
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    output
        .serialize()
        .unwrap()
        .remove(&ClientId::new(1))
        .unwrap_or_default()
}

#[test]
fn centered_agent_resizes_pty_and_aligns_frame_cursor_and_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (sender, receiver) = channels::unbounded();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        SenderWithContext::new(sender),
    );
    let id = PaneId::Terminal(TerminalId::new(1));
    tab.max_fullscreen_agent_width = 80;
    mark_agent(&mut tab, id);
    tab.handle_pty_bytes(TerminalId::new(1), b"Centered agent output".to_vec())
        .unwrap();
    let output = draw(&mut tab);
    let pane = tab.get_pane_with_id(id).unwrap();
    assert_eq!(pane.current_geom().cols.as_usize(), 121);
    assert_eq!(pane.render_geom().x, 19);
    assert_eq!(pane.render_geom().cols.as_usize(), 82);
    assert_eq!(pane.get_content_columns(), 80);
    assert_eq!(pane.get_content_x(), 20);
    assert_eq!(pane.get_content_rows(), 18);
    assert!(!pane.contains(&Position::new(2, 0)));
    assert!(pane.position_is_on_frame(&Position::new(2, 19)));
    assert!(!pane.position_is_on_frame(&Position::new(2, 20)));
    assert_eq!(
        pane.relative_position(&Position::new(2, 20)),
        Position::new(1, 0)
    );
    assert_eq!(
        tab.get_active_terminal_cursor_position(ClientId::new(1)),
        Some((41, 1, true))
    );
    assert!(receiver
        .try_iter()
        .any(|(instruction, _)| matches!(instruction,
        PtyWriteInstruction::ResizePty(pid, 80, 18, _, _) if pid == TerminalId::new(1))));
    assert_snapshot!(take_snapshot(
        &output,
        size.rows,
        size.cols,
        Palette::default()
    ));
    draw(&mut tab);
    assert!(receiver
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::ResizePty(..))));
}

#[test]
fn centered_agent_limit_tracks_identity_config_and_screen_size() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        ModeInfo::default(),
    );
    let id = PaneId::Terminal(TerminalId::new(1));
    tab.max_fullscreen_agent_width = 80;
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_columns(), 119);
    mark_agent(&mut tab, id);
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_columns(), 80);
    tab.set_pane_agent(id, None);
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_columns(), 80);
    for (columns, expected_width, expected_x) in [(122, 80, 20), (60, 58, 0), (120, 80, 19)] {
        tab.resize_whole_tab(Size {
            cols: columns,
            rows: 20,
        })
        .unwrap();
        draw(&mut tab);
        let pane = tab.get_pane_with_id(id).unwrap();
        assert_eq!(pane.get_content_columns(), expected_width);
        assert_eq!(pane.render_geom().x, expected_x);
    }
    for limit in [1, 100, usize::MAX, 0] {
        tab.max_fullscreen_agent_width = limit;
        draw(&mut tab);
        let pane = tab.get_pane_with_id(id).unwrap();
        assert_eq!(
            pane.get_content_columns(),
            if limit == 0 { 118 } else { limit.min(118) }
        );
    }
}

#[test]
fn centered_agent_split_fullscreen_and_close_preserve_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let agent = PaneId::Terminal(TerminalId::new(1));
    let other = PaneId::Terminal(TerminalId::new(2));
    tab.max_fullscreen_agent_width = 40;
    mark_agent(&mut tab, agent);
    draw(&mut tab);
    tab.new_pane(
        other,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    draw(&mut tab);
    let split = tab.get_pane_with_id(agent).unwrap().current_geom();
    assert_eq!(tab.get_pane_with_id(agent).unwrap().render_geom(), split);
    assert!(split.cols.as_usize() > 50);
    tab.toggle_pane_fullscreen(agent);
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        40
    );
    tab.unset_fullscreen();
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(agent).unwrap().render_geom(), split);
    tab.close_pane(other, false, None);
    draw(&mut tab);
    let pane = tab.get_pane_with_id(agent).unwrap();
    assert_eq!(pane.current_geom().cols.as_usize(), size.cols);
    assert_eq!(pane.get_content_columns(), 40);
}

#[test]
fn centered_agent_fullscreen_in_each_layer_and_frame_style() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client = ClientId::new(1);
    for style in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        for placement in [
            NewPanePlacement::default(),
            NewPanePlacement::Floating(None),
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
        ] {
            let mut tab = create_new_tab(size, ModeInfo::default());
            tab.set_pane_frames(style);
            let id = PaneId::Terminal(TerminalId::new(2));
            tab.new_pane(id, None, None, false, true, placement, Some(client), None)
                .unwrap();
            tab.max_fullscreen_agent_width = 40;
            mark_agent(&mut tab, id);
            draw(&mut tab);
            let original = tab.get_pane_with_id(id).unwrap().current_geom();
            assert_eq!(tab.get_pane_with_id(id).unwrap().render_geom(), original);
            tab.toggle_pane_fullscreen(id);
            draw(&mut tab);
            let pane = tab.get_pane_with_id(id).unwrap();
            assert_eq!(pane.get_content_columns(), 40);
            let frame = pane.render_geom();
            assert_eq!(frame.x, (size.cols - frame.cols.as_usize()) / 2);
            assert!(crate::tab::mouse_handler::MouseHandler::get_pane_at(
                &mut tab,
                &Position::new(3, 0),
                false
            )
            .unwrap()
            .is_none());
            tab.toggle_pane_no_ui_fullscreen(id);
            draw(&mut tab);
            let pane = tab.get_pane_with_id(id).unwrap();
            assert_eq!(pane.get_content_columns(), 40);
            assert_eq!(pane.get_content_rows(), size.rows);
            assert_eq!(pane.render_geom().cols.as_usize(), 40);
            tab.unset_fullscreen();
            draw(&mut tab);
            assert_eq!(tab.get_pane_with_id(id).unwrap().render_geom(), original);
        }
    }
}

#[test]
fn centered_agent_ignores_nonselectable_ui_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab_with_layout(
        size,
        ModeInfo::default(),
        r#"
        layout {
            pane size=1 borderless=true
            pane
            pane size=1 borderless=true
        }
    "#,
    );
    for terminal in [0, 2] {
        tab.get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(terminal)))
            .unwrap()
            .set_selectable(false);
    }
    let agent = PaneId::Terminal(TerminalId::new(1));
    mark_agent(&mut tab, agent);
    tab.max_fullscreen_agent_width = 80;
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        80
    );
    for terminal in [0, 2] {
        let pane = tab
            .get_pane_with_id(PaneId::Terminal(TerminalId::new(terminal)))
            .unwrap();
        assert_eq!(pane.render_geom(), pane.current_geom());
    }
}

#[test]
fn centered_agent_stack_is_not_a_single_pane() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        ModeInfo::default(),
    );
    let client = ClientId::new(1);
    let agent = PaneId::Terminal(TerminalId::new(1));
    tab.max_fullscreen_agent_width = 40;
    mark_agent(&mut tab, agent);
    draw(&mut tab);
    tab.new_stacked_pane(
        PaneId::Terminal(TerminalId::new(2)),
        None,
        None,
        false,
        true,
        Some(agent),
        Some(client),
        None,
        None,
    )
    .unwrap();
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(agent).unwrap().max_content_width(), 0);
    tab.toggle_pane_fullscreen(agent);
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        40
    );
    tab.unset_fullscreen();
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(agent).unwrap().max_content_width(), 0);
}

#[test]
fn centered_agent_pane_tabs_share_the_limit_and_restore_it() {
    use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsLayout};
    let (mut tab, receiver) = create_new_tab_with_plugin_receiver(
        Size {
            cols: 121,
            rows: 20,
        },
        ModeInfo::default(),
    );
    let client = ClientId::new(1);
    let agent = PaneId::Terminal(TerminalId::new(1));
    tab.max_fullscreen_agent_width = 40;
    mark_agent(&mut tab, agent);
    let layout = PaneTabsLayout {
        active: 0,
        tabs: ["Conversation", "Overview"]
            .map(|label| PaneTabDescriptor {
                label: label.into(),
                location: "yinz:fixture-plugin-for-tests".into(),
                initial_cwd: None,
                configuration: BTreeMap::new(),
            })
            .to_vec(),
    };
    let info = tab.mount_pane_tabs(agent, layout, 0, client).unwrap();
    draw(&mut tab);
    tab.select_pane_tab(info.id, 1, client).unwrap();
    let generation = receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::LoadPaneTab { generation, .. } => Some(generation),
            _ => None,
        })
        .unwrap();
    let plugin = crate::PluginId::new(101);
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:fixture-plugin-for-tests",
        &None,
        None,
        None,
    )
    .unwrap();
    tab.pane_tab_loaded(info.id, 1, generation, Ok((plugin, run)))
        .unwrap();
    draw(&mut tab);
    let panel = tab.get_pane_with_id(PaneId::Plugin(plugin)).unwrap();
    assert_eq!(panel.get_content_columns(), 40);
    assert_eq!(panel.render_geom().x, 39);
    tab.select_pane_tab(info.id, 0, client).unwrap();
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        40
    );
    tab.max_fullscreen_agent_width = 0;
    draw(&mut tab);
    for id in [agent, PaneId::Plugin(plugin)] {
        assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_columns(), 119);
    }
}

#[test]
fn centered_agent_does_not_narrow_children_sharing_a_fullscreen_panel() {
    use yinz_types::multipane::{MultipaneDirection, MultipaneSplit};
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        ModeInfo::default(),
    );
    let client = ClientId::new(1);
    let left = PaneId::Terminal(TerminalId::new(2));
    let right = PaneId::Terminal(TerminalId::new(3));
    for id in [left, right] {
        tab.new_pane(
            id,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(client),
            None,
        )
        .unwrap();
    }
    tab.create_multipane(
        [left, right],
        MultipaneSplit {
            direction: MultipaneDirection::LeftRight,
            ratio: 5000,
        },
        0,
        OverlayPlacement::dismissible(None),
    )
    .unwrap();
    mark_agent(&mut tab, left);
    tab.max_fullscreen_agent_width = 20;
    tab.toggle_pane_fullscreen(left);
    draw(&mut tab);
    for id in [left, right] {
        let pane = tab.get_pane_with_id(id).unwrap();
        assert_eq!(pane.render_geom(), pane.current_geom());
        assert_eq!(pane.max_content_width(), 0);
    }
}

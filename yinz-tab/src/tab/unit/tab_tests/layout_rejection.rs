use super::*;
use yinz_types::input::layout::Run;

#[test]
fn rejected_layout_preserves_panes_focus_and_swap_layout_state() {
    for override_existing in [false, true] {
        let mut tab = create_new_tab(Size { cols: 80, rows: 20 }, true);
        let client = ClientId::new(1);
        let original = PaneId::Terminal(TerminalId::new(1));
        tab.handle_pty_bytes(TerminalId::new(1), b"keep this output".to_vec())
            .unwrap();
        tab.swap_layouts.set_is_tiled_damaged();
        let geom = tab.get_pane_with_id(original).unwrap().current_geom();
        let swap_state = format!("{:?}", tab.swap_layouts);
        let layout = TiledPaneLayout {
            children: vec![
                TiledPaneLayout {
                    split_size: Some(SplitSize::Fixed(100)),
                    ..Default::default()
                },
                TiledPaneLayout::default(),
            ],
            ..Default::default()
        };
        let terminals = vec![(TerminalId::new(2), None), (TerminalId::new(3), None)];
        let result = if override_existing {
            tab.override_layout(
                layout,
                vec![],
                None,
                None,
                terminals,
                vec![],
                HashMap::new(),
                false,
                false,
                client,
                None,
            )
        } else {
            tab.apply_layout(
                layout,
                vec![],
                terminals,
                vec![],
                HashMap::new(),
                client,
                None,
            )
        };
        assert!(result.is_err(), "unfit layout must fail");
        assert_eq!(tab.get_active_pane_id(client), Some(original));
        let pane = tab.get_pane_with_id(original).unwrap();
        assert_eq!(pane.current_geom(), geom);
        assert!(pane.dump_screen(false, None).contains("keep this output"));
        assert_eq!(format!("{:?}", tab.swap_layouts), swap_state);
        assert_eq!(tab.pane_infos().len(), 1);
        assert!(!tab.is_pending());
        tab.assert_invariants();
    }
}

#[test]
fn rejected_landing_retains_adopted_terminals_and_their_buffered_output() {
    for (has_existing_pane, held) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut tab = create_new_tab(Size { cols: 80, rows: 20 }, true);
        let client = ClientId::new(1);
        let original = PaneId::Terminal(TerminalId::new(1));
        if !has_existing_pane {
            tab.extract_pane(original, true).unwrap();
        }
        tab.is_pending = true;
        tab.handle_pty_bytes(TerminalId::new(2), b"adopted output".to_vec())
            .unwrap();
        let command = RunCommand {
            command: PathBuf::from("codex"),
            args: vec!["--resume".into()],
            cwd: Some(PathBuf::from("/work")),
            hold_on_close: true,
            hold_on_start: held,
            use_terminal_title: true,
            ..Default::default()
        };
        tab.retain_adopted_layout_terminals(
            [TerminalId::new(2), TerminalId::new(3)]
                .into_iter()
                .map(|id| (id, command.clone(), held.then(|| command.clone())))
                .collect(),
            client,
        )
        .unwrap();
        assert!(!tab.is_pending());
        assert!(tab.pending_instructions.is_empty());
        assert!(tab
            .get_pane_with_id(PaneId::Terminal(TerminalId::new(2)))
            .unwrap()
            .dump_screen(false, None)
            .contains("adopted output"));
        assert!(tab
            .suppressed_panes
            .contains_key(&PaneId::Terminal(TerminalId::new(3))));
        for id in [TerminalId::new(2), TerminalId::new(3)] {
            let pane = tab.get_pane_with_id(PaneId::Terminal(id)).unwrap();
            assert_eq!(pane.invoked_with(), &Some(Run::Command(command.clone())));
            assert_eq!(pane.is_held(), held);
        }
        let focused = if has_existing_pane {
            original
        } else {
            PaneId::Terminal(TerminalId::new(2))
        };
        assert_eq!(tab.get_active_pane_id(client), Some(focused));
        assert!(tab.has_selectable_panes());
        tab.assert_invariants();
    }
}

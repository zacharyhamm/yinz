use super::*;

#[test]
fn click_on_plugin_highlight_sends_highlight_clicked() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let (mut tab, mock_plugin_receiver) =
        create_new_tab_with_plugin_receiver(size, ModeInfo::default());

    // Feed PTY bytes to pane 1
    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("click here foo bar".as_bytes()),
    )
    .unwrap();

    // Set plugin highlight on pane 1 for pattern "foo"
    let highlights = vec![RegexHighlight {
        pattern: "foo".into(),
        style: HighlightStyle::Emphasis0,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: true,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(42),
        highlights,
        &Style::default(),
    );

    // Alt+Click at position where "foo" appears
    // With draw_pane_frames=true, content starts at row 1, col 1
    // "click here foo bar" -> "foo" starts at offset 11, so col = 1 + 11 = 12
    let click_position = Position::new(1, 12);
    let mut alt_click = MouseEvent::new_left_press_event(click_position);
    alt_click.alt = true;
    let effect = tab.handle_mouse_event(&alt_click, client_id).unwrap();

    assert!(effect.state_changed);

    // Check that HighlightClicked was sent to the plugin channel
    let mut found_highlight_clicked = false;
    while let Ok((instruction, _ctx)) = mock_plugin_receiver.try_recv() {
        if let PluginInstruction::HighlightClicked {
            plugin_id,
            pane_id,
            pattern,
            matched_string,
            ..
        } = instruction
        {
            assert_eq!(plugin_id, crate::PluginId(42));
            assert_eq!(pane_id, PaneId::Terminal(crate::TerminalId(1)));
            assert_eq!(pattern, "foo");
            assert_eq!(matched_string, "foo");
            found_highlight_clicked = true;
            break;
        }
    }
    assert!(
        found_highlight_clicked,
        "Expected HighlightClicked instruction to be sent"
    );
}

#[test]
fn click_outside_highlight_starts_normal_selection() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let (mut tab, mock_plugin_receiver) =
        create_new_tab_with_plugin_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("click here foo bar".as_bytes()),
    )
    .unwrap();

    let highlights = vec![RegexHighlight {
        pattern: "foo".into(),
        style: HighlightStyle::Emphasis0,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: true,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(42),
        highlights,
        &Style::default(),
    );

    // Click on "click" (col 1), not on "foo"
    let click_position = Position::new(1, 1);
    let _effect = tab
        .handle_mouse_event(&MouseEvent::new_left_press_event(click_position), client_id)
        .unwrap();

    // No HighlightClicked should be sent
    let mut found_highlight_clicked = false;
    while let Ok((instruction, _ctx)) = mock_plugin_receiver.try_recv() {
        if matches!(instruction, PluginInstruction::HighlightClicked { .. }) {
            found_highlight_clicked = true;
        }
    }
    assert!(
        !found_highlight_clicked,
        "HighlightClicked should NOT be sent for miss"
    );

    // Selection should have started
    assert!(tab.selecting_with_mouse_in_pane.is_some());
}

#[test]
fn hover_over_highlight_shows_styling() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("hello link_text bar".as_bytes()),
    )
    .unwrap();

    let highlights = vec![RegexHighlight {
        pattern: "link_text".into(),
        style: HighlightStyle::None,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: true,
        bold: false,
        italic: true,
        underline: true,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(1),
        highlights,
        &Style::default(),
    );

    // Hover over the match (link_text starts at offset 6, with frame offset col = 7)
    let hover_position = Position::new(1, 9);
    let _effect = tab
        .handle_mouse_event(
            &MouseEvent::new_buttonless_motion(hover_position),
            client_id,
        )
        .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_with_hover = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot_with_hover);

    // Move hover away from the match
    let mut output2 = Output::default();
    let far_position = Position::new(1, 1);
    let _effect2 = tab
        .handle_mouse_event(&MouseEvent::new_buttonless_motion(far_position), client_id)
        .unwrap();

    tab.render(&mut output2, None).unwrap();
    let snapshot_without_hover = take_snapshot(
        output2.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot_without_hover);
}

#[test]
fn hover_on_unfocused_pane_no_highlight() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId(2));
    let mut output = Output::default();

    // Create a vertical split
    tab.vertical_split(new_pane_id, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId(1), Vec::from("link_text".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId(2), Vec::from("right pane".as_bytes()))
        .unwrap();

    // Active pane is pane 2 (right)
    assert_eq!(
        tab.get_active_pane_id(client_id),
        Some(PaneId::Terminal(crate::TerminalId(2)))
    );

    // Set on_hover highlight on pane 1 (left, unfocused)
    let highlights = vec![RegexHighlight {
        pattern: "link_text".into(),
        style: HighlightStyle::None,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: true,
        bold: false,
        italic: true,
        underline: true,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(1),
        highlights,
        &Style::default(),
    );

    // Hover over left pane (unfocused)
    let hover_position = Position::new(1, 5);
    let _effect = tab
        .handle_mouse_event(
            &MouseEvent::new_buttonless_motion(hover_position),
            client_id,
        )
        .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    // Hover highlight should NOT be visible on the unfocused pane
    assert_snapshot!(snapshot);
}

#[test]
fn highlight_click_in_mouse_mode_pane_suppressed() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let (mut tab, mock_plugin_receiver) =
        create_new_tab_with_plugin_receiver(size, ModeInfo::default());

    // Feed PTY bytes that enable mouse tracking (SGR mode + any-event tracking)
    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("\x1b[?1006h\x1b[?1003h".as_bytes()),
    )
    .unwrap();
    // Then feed text with matchable content
    tab.handle_pty_bytes(crate::TerminalId(1), Vec::from("foo bar baz".as_bytes()))
        .unwrap();

    // Set plugin highlight
    let highlights = vec![RegexHighlight {
        pattern: "foo".into(),
        style: HighlightStyle::Emphasis0,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: true,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(42),
        highlights,
        &Style::default(),
    );

    // Left-click on the highlight position
    // With mouse tracking, the click goes through SendToTerminal, not StartSelection
    let click_position = Position::new(1, 1);
    let _effect = tab
        .handle_mouse_event(&MouseEvent::new_left_press_event(click_position), client_id)
        .unwrap();

    // No HighlightClicked should be sent (the click is forwarded to the terminal)
    let mut found_highlight_clicked = false;
    while let Ok((instruction, _ctx)) = mock_plugin_receiver.try_recv() {
        if matches!(instruction, PluginInstruction::HighlightClicked { .. }) {
            found_highlight_clicked = true;
        }
    }
    assert!(
        !found_highlight_clicked,
        "HighlightClicked should NOT be sent when terminal has mouse tracking"
    );
}

#[test]
fn set_and_clear_highlights_across_tiled_and_floating() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let pane2 = PaneId::Terminal(crate::TerminalId(2));
    let pane3 = PaneId::Terminal(crate::TerminalId(3));

    // Create a second tiled pane
    tab.vertical_split(pane2, None, client_id, None, None)
        .unwrap();

    // Create a floating pane
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        pane3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    // Feed content to all panes
    tab.handle_pty_bytes(crate::TerminalId(1), Vec::from("aaa bbb".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId(2), Vec::from("aaa bbb".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId(3), Vec::from("aaa bbb".as_bytes()))
        .unwrap();

    // Set highlights from plugin 1 (pattern "aaa") on all panes
    let h1 = vec![RegexHighlight {
        pattern: "aaa".into(),
        style: HighlightStyle::Emphasis0,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: true,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(1),
        h1.clone(),
        &Style::default(),
    );
    tab.set_plugin_regex_highlights_for_pane(
        pane2,
        crate::PluginId(1),
        h1.clone(),
        &Style::default(),
    );
    tab.set_plugin_regex_highlights_for_pane(
        pane3,
        crate::PluginId(1),
        h1.clone(),
        &Style::default(),
    );

    // Set highlights from plugin 2 (pattern "bbb") on all panes
    let h2 = vec![RegexHighlight {
        pattern: "bbb".into(),
        style: HighlightStyle::Emphasis1,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: false,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(2),
        h2.clone(),
        &Style::default(),
    );
    tab.set_plugin_regex_highlights_for_pane(
        pane2,
        crate::PluginId(2),
        h2.clone(),
        &Style::default(),
    );
    tab.set_plugin_regex_highlights_for_pane(
        pane3,
        crate::PluginId(2),
        h2.clone(),
        &Style::default(),
    );

    // Clear plugin 1 highlights across all panes
    tab.clear_all_plugin_highlights(crate::PluginId(1));

    // Verify plugin 2 highlights remain — we can use the tab render to snapshot,
    // or directly check that plugin 2 pattern still matches on pane 1
    // Since clear_all_plugin_highlights iterates all panes, we test via
    // the set_plugin_regex_highlights_for_pane / clear flow:
    // Plugin 2 "bbb" highlights should still be active.
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn plain_click_on_highlight_does_not_send_highlight_clicked() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let (mut tab, mock_plugin_receiver) =
        create_new_tab_with_plugin_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("click here foo bar".as_bytes()),
    )
    .unwrap();

    let highlights = vec![RegexHighlight {
        pattern: "foo".into(),
        style: HighlightStyle::Emphasis0,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: true,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(42),
        highlights,
        &Style::default(),
    );

    // Plain left-click on "foo" — should start selection, NOT send HighlightClicked
    let click_position = Position::new(1, 12);
    let _effect = tab
        .handle_mouse_event(&MouseEvent::new_left_press_event(click_position), client_id)
        .unwrap();

    let mut found_highlight_clicked = false;
    while let Ok((instruction, _ctx)) = mock_plugin_receiver.try_recv() {
        if matches!(instruction, PluginInstruction::HighlightClicked { .. }) {
            found_highlight_clicked = true;
        }
    }
    assert!(
        !found_highlight_clicked,
        "Plain click should NOT send HighlightClicked after Alt+Click change"
    );

    // Selection should have started instead
    assert!(tab.selecting_with_mouse_in_pane.is_some());
}

#[test]
fn alt_click_outside_highlight_falls_through_to_group_toggle() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let (mut tab, mock_plugin_receiver) =
        create_new_tab_with_plugin_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("click here foo bar".as_bytes()),
    )
    .unwrap();

    let highlights = vec![RegexHighlight {
        pattern: "foo".into(),
        style: HighlightStyle::Emphasis0,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: true,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(42),
        highlights,
        &Style::default(),
    );

    // Alt+Click on "click" (col 1), not on "foo" — should fall through to group toggle
    let click_position = Position::new(1, 1);
    let mut alt_click = MouseEvent::new_left_press_event(click_position);
    alt_click.alt = true;
    let effect = tab.handle_mouse_event(&alt_click, client_id).unwrap();

    // No HighlightClicked should be sent
    let mut found_highlight_clicked = false;
    while let Ok((instruction, _ctx)) = mock_plugin_receiver.try_recv() {
        if matches!(instruction, PluginInstruction::HighlightClicked { .. }) {
            found_highlight_clicked = true;
        }
    }
    assert!(
        !found_highlight_clicked,
        "Alt+Click outside highlight should NOT send HighlightClicked"
    );

    // Group toggle effect should have been produced
    assert_eq!(
        effect.group_toggle,
        Some(PaneId::Terminal(crate::TerminalId(1)))
    );
}

#[test]
fn hover_over_highlight_with_tooltip_caches_tooltip() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("hello link_text bar".as_bytes()),
    )
    .unwrap();

    let highlights = vec![RegexHighlight {
        pattern: "link_text".into(),
        style: HighlightStyle::None,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: true,
        bold: false,
        italic: true,
        underline: true,
        tooltip_text: Some("Open".to_string()),
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(1),
        highlights,
        &Style::default(),
    );

    // Hover over the match (link_text starts at offset 6, with frame offset col = 7)
    let hover_position = Position::new(1, 9);
    let _effect = tab
        .handle_mouse_event(
            &MouseEvent::new_buttonless_motion(hover_position),
            client_id,
        )
        .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_with_tooltip = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    // The bottom frame row should contain the tooltip text
    assert_snapshot!(snapshot_with_tooltip);

    // Move hover away — tooltip should disappear
    let mut output2 = Output::default();
    let far_position = Position::new(1, 1);
    let _effect2 = tab
        .handle_mouse_event(&MouseEvent::new_buttonless_motion(far_position), client_id)
        .unwrap();

    tab.render(&mut output2, None).unwrap();
    let snapshot_without_tooltip = take_snapshot(
        output2.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot_without_tooltip);
}

// =====================================================================
// Highlight Layer Priority Integration Tests
// =====================================================================

#[test]
fn higher_layer_highlight_style_wins_in_rendered_output() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("overlap_text here\n".as_bytes()),
    )
    .unwrap();

    // Plugin 1 (id: 10): Hint layer, underline only
    let h1 = vec![RegexHighlight {
        pattern: "overlap_text".into(),
        style: HighlightStyle::Emphasis0,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: true,
        tooltip_text: None,
    }];
    // Plugin 2 (id: 20): Tool layer, bold only
    let h2 = vec![RegexHighlight {
        pattern: "overlap_text".into(),
        style: HighlightStyle::Emphasis1,
        layer: HighlightLayer::Tool,
        context: BTreeMap::new(),
        on_hover: false,
        bold: true,
        italic: false,
        underline: false,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(10),
        h1,
        &Style::default(),
    );
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(20),
        h2,
        &Style::default(),
    );

    tab.render(&mut output, None).unwrap();
    let raw_output = output.serialize().unwrap();
    let raw_str = raw_output.get(&client_id).unwrap();

    // The renderer emits a series of individual SGR codes before the
    // highlighted text.  Extract the full escape prefix preceding
    // "overlap_text" and verify that bold (\x1b[1m) is present and
    // underline (\x1b[4m) is absent.
    let idx = raw_str
        .find("overlap_text")
        .expect("overlap_text should appear in raw output");
    let preceding = &raw_str[..idx];
    // Walk backwards to find the reset (\x1b[m) that starts this cell's styling
    let reset_pos = preceding
        .rfind("\x1b[m")
        .expect("should find a reset before overlap_text");
    let sgr_block = &raw_str[reset_pos..idx];

    assert!(
        sgr_block.contains("\x1b[1m"),
        "Tool layer bold styling (\\x1b[1m) should be present before overlap_text, got: {:?}",
        sgr_block
    );
    assert!(
        !sgr_block.contains("\x1b[4m"),
        "Hint layer underline styling (\\x1b[4m) should NOT be present before overlap_text, got: {:?}",
        sgr_block
    );
}

#[test]
fn click_dispatches_to_higher_layer_plugin() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let (mut tab, mock_plugin_receiver) =
        create_new_tab_with_plugin_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("click here shared_text bar\n".as_bytes()),
    )
    .unwrap();

    // Plugin 1 (id: 10): Hint layer
    let h1 = vec![RegexHighlight {
        pattern: "shared_text".into(),
        style: HighlightStyle::Emphasis0,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: true,
        tooltip_text: None,
    }];
    // Plugin 2 (id: 20): Tool layer
    let h2 = vec![RegexHighlight {
        pattern: "shared_text".into(),
        style: HighlightStyle::Emphasis1,
        layer: HighlightLayer::Tool,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: true,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(10),
        h1,
        &Style::default(),
    );
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(20),
        h2,
        &Style::default(),
    );

    // Alt+Click on "shared_text" (starts at offset 11, frame adds +1 => col 12)
    let click_position = Position::new(1, 12);
    let mut alt_click = MouseEvent::new_left_press_event(click_position);
    alt_click.alt = true;
    let _effect = tab.handle_mouse_event(&alt_click, client_id).unwrap();

    // Drain the plugin receiver and find the HighlightClicked instruction
    let mut found_plugin_id = None;
    while let Ok((instruction, _ctx)) = mock_plugin_receiver.try_recv() {
        if let PluginInstruction::HighlightClicked { plugin_id, .. } = instruction {
            found_plugin_id = Some(plugin_id);
        }
    }
    assert_eq!(
        found_plugin_id,
        Some(zellij_utils::internal_ids::PluginId(20)),
        "HighlightClicked should be dispatched to the Tool layer plugin (id: 20)"
    );
}

#[test]
fn hover_tooltip_shows_higher_layer_tooltip() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("hover here tooltipped bar\n".as_bytes()),
    )
    .unwrap();

    // Plugin 1 (id: 10): Hint layer with tooltip
    let h1 = vec![RegexHighlight {
        pattern: "tooltipped".into(),
        style: HighlightStyle::None,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: true,
        bold: false,
        italic: true,
        underline: true,
        tooltip_text: Some("Hint Tooltip".to_string()),
    }];
    // Plugin 2 (id: 20): Tool layer with tooltip
    let h2 = vec![RegexHighlight {
        pattern: "tooltipped".into(),
        style: HighlightStyle::None,
        layer: HighlightLayer::Tool,
        context: BTreeMap::new(),
        on_hover: true,
        bold: false,
        italic: true,
        underline: true,
        tooltip_text: Some("Tool Tooltip".to_string()),
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(10),
        h1,
        &Style::default(),
    );
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(20),
        h2,
        &Style::default(),
    );

    // Hover over "tooltipped" (starts at offset 11, frame adds +1 => col 12)
    let hover_position = Position::new(1, 12);
    let _effect = tab
        .handle_mouse_event(
            &MouseEvent::new_buttonless_motion(hover_position),
            client_id,
        )
        .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    // The bottom frame row should contain "Tool Tooltip" from the Tool layer
    assert_snapshot!(snapshot);
}

#[test]
fn non_overlapping_highlights_from_different_layers_coexist() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    tab.handle_pty_bytes(crate::TerminalId(1), Vec::from("aaa bbb ccc\n".as_bytes()))
        .unwrap();

    // Plugin 1 (id: 10): Hint layer, underline on "aaa"
    let h1 = vec![RegexHighlight {
        pattern: "aaa".into(),
        style: HighlightStyle::None,
        layer: HighlightLayer::Hint,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: false,
        underline: true,
        tooltip_text: None,
    }];
    // Plugin 2 (id: 20): Tool layer, bold on "bbb"
    let h2 = vec![RegexHighlight {
        pattern: "bbb".into(),
        style: HighlightStyle::None,
        layer: HighlightLayer::Tool,
        context: BTreeMap::new(),
        on_hover: false,
        bold: true,
        italic: false,
        underline: false,
        tooltip_text: None,
    }];
    // Plugin 3 (id: 30): ActionFeedback layer, italic on "ccc"
    let h3 = vec![RegexHighlight {
        pattern: "ccc".into(),
        style: HighlightStyle::None,
        layer: HighlightLayer::ActionFeedback,
        context: BTreeMap::new(),
        on_hover: false,
        bold: false,
        italic: true,
        underline: false,
        tooltip_text: None,
    }];
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(10),
        h1,
        &Style::default(),
    );
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(20),
        h2,
        &Style::default(),
    );
    tab.set_plugin_regex_highlights_for_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        crate::PluginId(30),
        h3,
        &Style::default(),
    );

    tab.render(&mut output, None).unwrap();
    let raw_output = output.serialize().unwrap();
    let raw_str = raw_output.get(&client_id).unwrap();

    // The renderer emits individual SGR codes in sequence before each
    // highlighted word.  Extract the escape prefix block before each word
    // and verify the expected attribute is present.

    // Helper: extract the SGR block between the last \x1b[m reset and the text
    let sgr_before = |text: &str| -> String {
        let idx = raw_str
            .find(text)
            .unwrap_or_else(|| panic!("{} not found in raw output", text));
        let preceding = &raw_str[..idx];
        let reset_pos = preceding
            .rfind("\x1b[m")
            .unwrap_or_else(|| panic!("no reset before {}", text));
        raw_str[reset_pos..idx].to_string()
    };

    let aaa_sgr = sgr_before("aaa");
    let bbb_sgr = sgr_before("bbb");
    let ccc_sgr = sgr_before("ccc");

    assert!(
        aaa_sgr.contains("\x1b[4m"),
        "Hint layer underline (\\x1b[4m) should be applied to 'aaa', got: {:?}",
        aaa_sgr
    );
    assert!(
        bbb_sgr.contains("\x1b[1m"),
        "Tool layer bold (\\x1b[1m) should be applied to 'bbb', got: {:?}",
        bbb_sgr
    );
    assert!(
        ccc_sgr.contains("\x1b[3m"),
        "ActionFeedback layer italic (\\x1b[3m) should be applied to 'ccc', got: {:?}",
        ccc_sgr
    );
}

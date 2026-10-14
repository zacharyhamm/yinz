use super::*;
use crate::ui::damage::CellRect;

fn paint(tab: &mut Tab, terminal: u32, letter: char) {
    let pane = tab
        .get_pane_with_id(PaneId::Terminal(TerminalId::new(terminal)))
        .unwrap();
    let bytes: String = (1..=pane.get_content_rows())
        .map(|row| {
            format!(
                "\x1b[{row};1H{}",
                letter.to_string().repeat(pane.get_content_columns())
            )
        })
        .collect();
    tab.handle_pty_bytes(
        TerminalId::new(terminal),
        format!("\x1b[?7l{bytes}\x1b[?25l").into_bytes(),
    )
    .unwrap();
}
fn frame(tab: &mut Tab, history: &mut String) -> String {
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    history.push_str(&output.serialize().unwrap()[&ClientId::new(1)]);
    take_snapshot(history, 20, 60, Palette::default())
}

#[test]
fn terminal_overlay_inherits_global_background_opacity() {
    let mut tab =
        create_new_tab_without_pane_frames(Size { cols: 60, rows: 20 }, ModeInfo::default());
    tab.new_pane(
        PaneId::Terminal(TerminalId::new(2)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement {
            coordinates: FloatingPaneCoordinates::new(
                Some("10%".into()),
                Some("10%".into()),
                Some("80%".into()),
                Some("80%".into()),
                None,
                None,
            ),
            background_opacity: Some(yinz_types::input::options::DEFAULT_BACKGROUND_OPACITY),
            ..Default::default()
        }),
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
    paint(&mut tab, 1, 'a');
    tab.handle_pty_bytes(TerminalId::new(2), b"\x1b[?25lOVERLAY".to_vec())
        .unwrap();
    let mut history = String::new();
    let default = frame(&mut tab, &mut history);
    assert!(default.contains("OVERLAYaaaaaaa"), "{default}");

    tab.update_background_opacity(100);
    let opaque = frame(&mut tab, &mut history);
    assert!(opaque.contains("OVERLAY       "), "{opaque}");

    tab.update_background_opacity(50);
    let translucent = frame(&mut tab, &mut history);
    assert!(translucent.contains("OVERLAYaaaaaaa"), "{translucent}");
}

#[test]
fn translucent_overlay_refreshes_both_layers_and_restores_old_bounds() {
    let mut tab =
        create_new_tab_without_pane_frames(Size { cols: 60, rows: 20 }, ModeInfo::default());
    let overlay = PaneId::Terminal(TerminalId::new(2));
    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(5)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(20)),
        height: Some(PercentOrFixed::Fixed(8)),
        borderless: Some(true),
        ..Default::default()
    };
    tab.new_pane(
        overlay,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement {
            background_opacity: Some(75),
            coordinates: Some(coordinates.clone()),
            ..Default::default()
        }),
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
    paint(&mut tab, 1, 'a');
    tab.handle_pty_bytes(TerminalId::new(2), b"\x1b[?25lLABEL".to_vec())
        .unwrap();
    let mut history = String::new();
    let first = frame(&mut tab, &mut history);
    assert!(first.contains("LABELaaaaaaaa"), "{first}");
    paint(&mut tab, 1, 'b');
    let updated = frame(&mut tab, &mut history);
    assert!(
        updated.contains("LABELbbbbbbbb"),
        "idle overlay must reveal new output: {updated}"
    );
    tab.handle_pty_bytes(TerminalId::new(2), b"\x1b[2J".to_vec())
        .unwrap();
    let erased = frame(&mut tab, &mut history);
    assert!(!erased.contains("LABEL"));
    assert!(
        erased.contains(&"b".repeat(60)),
        "erased overlay cells must reveal idle pane: {erased}"
    );
    tab.handle_pty_bytes(TerminalId::new(2), b"\x1b[HLABEL".to_vec())
        .unwrap();
    frame(&mut tab, &mut history);
    tab.change_overlay_coordinates_batch(
        vec![(
            overlay,
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(30)),
                width: Some(PercentOrFixed::Fixed(25)),
                ..coordinates
            },
        )],
        true,
    )
    .unwrap();
    let moved = frame(&mut tab, &mut history);
    assert_eq!(moved.matches("LABEL").count(), 1, "{moved}");
    assert!(
        moved.contains(&format!("{}LABEL", "b".repeat(30))),
        "{moved}"
    );
    tab.hide_overlay_pane(overlay);
    let hidden = frame(&mut tab, &mut history);
    assert!(!hidden.contains("LABEL"));
    tab.show_overlay_pane(overlay);
    tab.update_background_opacity(100);
    let opaque = frame(&mut tab, &mut history);
    assert!(opaque.contains("LABEL                    "), "{opaque}");
    tab.update_background_opacity(85);
    assert!(frame(&mut tab, &mut history).contains("LABELbbbbbbbb"));
    tab.update_background_opacity(0);
    assert!(frame(&mut tab, &mut history).contains("LABELbbbbbbbb"));
}

#[test]
fn translucent_plugin_cells_are_composed_independently_for_each_client() {
    use yinz_types::internal_ids::PluginId;
    let (mut tab, _receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 60, rows: 20 }, ModeInfo::default());
    tab.set_pane_frames(PaneFrameStyle::None);
    let clients = [ClientId::new(1), ClientId::new(2)];
    tab.add_client(clients[1], None).unwrap();
    let plugin = PluginId::new(9);
    tab.new_pane(
        PaneId::Plugin(plugin),
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement {
            background_opacity: Some(50),
            coordinates: Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(5)),
                y: Some(PercentOrFixed::Fixed(5)),
                width: Some(PercentOrFixed::Fixed(20)),
                height: Some(PercentOrFixed::Fixed(8)),
                borderless: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        Some(clients[0]),
        None,
    )
    .unwrap();
    for (client, label) in clients.into_iter().zip(["FIRST", "OTHER"]) {
        tab.handle_plugin_bytes(plugin, client, label.as_bytes().to_vec())
            .unwrap();
    }
    for letter in ['a', 'b'] {
        paint(&mut tab, 1, letter);
        let mut output = Output::default();
        tab.render(&mut output, None).unwrap();
        let bytes = output.serialize().unwrap();
        for (client, own, other) in [
            (clients[0], "FIRST", "OTHER"),
            (clients[1], "OTHER", "FIRST"),
        ] {
            let snapshot = take_snapshot(&bytes[&client], 20, 60, Palette::default());
            assert!(
                snapshot.contains(&format!("{own}{}", letter.to_string().repeat(5))),
                "{snapshot}"
            );
            assert!(!snapshot.contains(other), "{snapshot}");
        }
        // Each plugin client has its own completed grid, even when both are locked.
        for client in clients {
            tab.handle_plugin_bytes(plugin, client, b"\x1b[?2026hUNFINISHED".to_vec())
                .unwrap();
        }
    }
    for (client, label) in clients.into_iter().zip(["NEW_A", "NEW_B"]) {
        tab.handle_plugin_bytes(plugin, client, format!("{label}\x1b[?2026l").into_bytes())
            .unwrap();
    }
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let bytes = output.serialize().unwrap();
    for (client, label) in clients.into_iter().zip(["NEW_A", "NEW_B"]) {
        let snapshot = take_snapshot(&bytes[&client], 20, 60, Palette::default());
        assert!(snapshot.contains(&format!("{label}bbbbb")), "{snapshot}");
    }
}

fn add_translucent_overlay(tab: &mut Tab, terminal: u32, x: usize) {
    tab.new_pane(
        PaneId::Terminal(TerminalId::new(terminal)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement {
            background_opacity: Some(50),
            coordinates: Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(x)),
                y: Some(PercentOrFixed::Fixed(5)),
                width: Some(PercentOrFixed::Fixed(20)),
                height: Some(PercentOrFixed::Fixed(8)),
                borderless: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
}

#[test]
fn translucent_cursor_cell_restores_transparency_when_moved_hidden_or_unfocused() {
    let mut tab =
        create_new_tab_without_pane_frames(Size { cols: 60, rows: 20 }, ModeInfo::default());
    paint(&mut tab, 1, 'a');
    add_translucent_overlay(&mut tab, 2, 5);
    let mut history = String::new();
    for (bytes, expected) in [
        ("LABEL\x1b[?25h", "LABEL aaaa"),
        ("\x1b[C", "LABELa aaa"),
        ("\x1b[?25l", "LABELaaaaa"),
        ("\x1b[?25h", "LABELa aaa"),
        ("\x1b[?2026h\x1b[C", "LABELaaaaa"),
        ("\x1b[?2026l", "LABELaa aa"),
    ] {
        tab.handle_pty_bytes(TerminalId::new(2), bytes.as_bytes().to_vec())
            .unwrap();
        let snapshot = frame(&mut tab, &mut history);
        assert!(snapshot.contains(expected), "{bytes:?}: {snapshot}");
    }

    let client = ClientId::new(1);
    tab.focus_pane_with_id(PaneId::Terminal(TerminalId::new(1)), false, false, client)
        .unwrap();
    let unfocused = frame(&mut tab, &mut history);
    assert!(unfocused.contains("LABELaaaaa"), "{unfocused}");
    tab.focus_pane_with_id(PaneId::Terminal(TerminalId::new(2)), false, false, client)
        .unwrap();
    let focused = frame(&mut tab, &mut history);
    assert!(focused.contains("LABELaa aa"), "{focused}");

    let (x, y, true) = tab.get_active_terminal_cursor_position(client).unwrap() else {
        panic!("expected a visible cursor");
    };
    let mut output = Output::default();
    output.set_context_menu_rects(HashMap::from([(
        client,
        vec![CellRect {
            x,
            y,
            cols: 1,
            rows: 1,
        }],
    )]));
    tab.render(&mut output, None).unwrap();
    let bytes = output.serialize().unwrap();
    assert!(!bytes[&client].contains("\x1b[?25h"));
    history.push_str(&bytes[&client]);
    let occluded = take_snapshot(&history, 20, 60, Palette::default());
    assert!(occluded.contains("LABELaaaaa"), "{occluded}");
}

#[test]
fn translucent_plugin_cursor_cells_follow_each_clients_cursor() {
    use yinz_types::internal_ids::PluginId;
    let (mut tab, _receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 60, rows: 20 }, ModeInfo::default());
    tab.set_pane_frames(PaneFrameStyle::None);
    let clients = [ClientId::new(1), ClientId::new(2)];
    tab.add_client(clients[1], None).unwrap();
    let plugin = PluginId::new(9);
    tab.new_pane(
        PaneId::Plugin(plugin),
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement {
            background_opacity: Some(50),
            coordinates: Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(5)),
                y: Some(PercentOrFixed::Fixed(5)),
                width: Some(PercentOrFixed::Fixed(20)),
                height: Some(PercentOrFixed::Fixed(8)),
                borderless: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        Some(clients[0]),
        None,
    )
    .unwrap();
    paint(&mut tab, 1, 'a');
    for (client, cursor_column) in clients.into_iter().zip([5, 7]) {
        tab.focus_pane_with_id(PaneId::Plugin(plugin), false, false, client)
            .unwrap();
        tab.handle_plugin_bytes(plugin, client, b"LABEL".to_vec())
            .unwrap();
        tab.show_plugin_cursor(plugin, client, Some((cursor_column, 0)));
    }
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let bytes = output.serialize().unwrap();
    for (client, expected) in clients.into_iter().zip(["LABEL aaaa", "LABELaa aa"]) {
        let snapshot = take_snapshot(&bytes[&client], 20, 60, Palette::default());
        assert!(snapshot.contains(expected), "{client}: {snapshot}");
    }
}

#[test]
fn translucent_overlay_created_during_sync_keeps_completed_text_until_unlock() {
    let mut tab =
        create_new_tab_without_pane_frames(Size { cols: 60, rows: 20 }, ModeInfo::default());
    paint(&mut tab, 1, 'a');
    let mut history = String::new();
    frame(&mut tab, &mut history);
    tab.handle_pty_bytes(TerminalId::new(1), b"\x1b[?2026h".to_vec())
        .unwrap();
    paint(&mut tab, 1, 'b');
    add_translucent_overlay(&mut tab, 2, 5);
    for label in ["FIRST", "OTHER", "THIRD"] {
        tab.handle_pty_bytes(TerminalId::new(2), format!("\x1b[H{label}").into_bytes())
            .unwrap();
        let snapshot = frame(&mut tab, &mut history);
        assert!(
            snapshot.contains(&format!("{label} aaaaaaaa")),
            "{snapshot}"
        );
        assert!(!snapshot.contains("bbbb"), "{snapshot}");
    }
    tab.change_overlay_coordinates_batch(
        vec![(
            PaneId::Terminal(TerminalId::new(2)),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(30)),
                width: Some(PercentOrFixed::Fixed(25)),
                ..Default::default()
            },
        )],
        true,
    )
    .unwrap();
    let moved = frame(&mut tab, &mut history);
    assert!(
        moved.contains(&format!("{}THIRD aaaaaaaa", "a".repeat(30))),
        "{moved}"
    );
    tab.handle_pty_bytes(TerminalId::new(1), b"\x1b[?2026l".to_vec())
        .unwrap();
    let unlocked = frame(&mut tab, &mut history);
    assert!(unlocked.contains("THIRD bbbbbbbb"), "{unlocked}");
    assert!(!unlocked.contains("aaaa"), "{unlocked}");
}

#[test]
fn translucent_overlay_beyond_original_fullscreen_bounds_retains_idle_text() {
    for floating in [false, true] {
        let mut tab =
            create_new_tab_without_pane_frames(Size { cols: 60, rows: 20 }, ModeInfo::default());
        let second = PaneId::Terminal(TerminalId::new(2));
        let fullscreen = if floating {
            tab.new_floating_pane(
                second,
                None,
                None,
                false,
                true,
                Some(FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(0)),
                    y: Some(PercentOrFixed::Fixed(0)),
                    width: Some(PercentOrFixed::Fixed(25)),
                    height: Some(PercentOrFixed::Fixed(10)),
                    borderless: Some(true),
                    ..Default::default()
                }),
                None,
            )
            .unwrap();
            second
        } else {
            tab.vertical_split(second, None, None, ClientId::new(1), None, None)
                .unwrap();
            PaneId::Terminal(TerminalId::new(1))
        };
        let original = tab
            .get_pane_with_id(fullscreen)
            .unwrap()
            .position_and_size();
        assert!(original.x + original.cols.as_usize() <= 35);
        tab.toggle_pane_fullscreen(fullscreen);
        let mut history = String::new();
        paint(&mut tab, if floating { 2 } else { 1 }, 'a');
        frame(&mut tab, &mut history);
        add_translucent_overlay(&mut tab, 3, 35);
        tab.handle_pty_bytes(TerminalId::new(3), b"LABEL".to_vec())
            .unwrap();
        for _ in 0..3 {
            let snapshot = frame(&mut tab, &mut history);
            assert!(
                snapshot.contains("LABEL aaaaaaaa"),
                "floating={floating}: {snapshot}"
            );
        }
    }
}

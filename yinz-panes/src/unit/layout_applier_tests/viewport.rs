use super::*;
use yinz_runtime::plugin_location::run_plugin_or_alias_from_url;

fn viewport_after_applying_layout_with_plugins(
    kdl_layout: &str,
    terminal_ids: Vec<(TerminalId, Option<RunCommand>)>,
    plugin_ids: Vec<(&str, u32)>,
    size: Size,
) -> Viewport {
    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let mut new_plugin_ids = HashMap::new();
    for (location, plugin_id) in plugin_ids {
        let plugin = run_plugin_or_alias_from_url(location, &None, None, None).unwrap();
        new_plugin_ids
            .entry(plugin)
            .or_insert_with(Vec::new)
            .push(crate::PluginId::new(plugin_id));
    }
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    ) = create_layout_applier_fixtures(size);
    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );
    applier
        .apply_layout(
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![(TerminalId::new(99), None)],
            new_plugin_ids,
            ClientId::new(1),
        )
        .unwrap();
    let expected = *viewport.borrow();
    let floating_geom = floating_panes
        .get_panes()
        .next()
        .unwrap()
        .1
        .position_and_size();
    assert_eq!(
        (floating_geom.x, floating_geom.y),
        (expected.cols / 4, expected.rows / 4)
    );
    assert_eq!(floating_geom.cols.as_usize(), expected.cols / 2);
    assert_eq!(floating_geom.rows.as_usize(), expected.rows / 2);
    // Reset and recompute to cover live updates as well as prepared placement.
    LayoutApplier::offset_viewport(
        viewport.clone(),
        display_area,
        &mut tiled_panes,
        draw_pane_frames,
    );
    assert_eq!(*viewport.borrow(), expected);
    expected
}

#[test]
fn complete_ui_strips_trim_each_edge_and_prepared_floating_placement() {
    let row = r#"pane size=2 split_direction="vertical" {
        pane borderless=true { plugin location="yinz:compact-bar"; }
        pane borderless=true { plugin location="yinz:status-bar"; }
    }"#;
    let col = row
        .replace("size=2", "size=10")
        .replace("vertical", "horizontal");
    for (body, expected) in [
        (
            format!("{row}\npane"),
            Viewport {
                x: 0,
                y: 2,
                rows: 38,
                cols: 120,
            },
        ),
        (
            format!("pane\n{row}"),
            Viewport {
                x: 0,
                y: 0,
                rows: 38,
                cols: 120,
            },
        ),
        (
            format!("pane split_direction=\"vertical\" {{ {col}\npane; }}"),
            Viewport {
                x: 10,
                y: 0,
                rows: 40,
                cols: 110,
            },
        ),
        (
            format!("pane split_direction=\"vertical\" {{ pane; {col}\n}}"),
            Viewport {
                x: 0,
                y: 0,
                rows: 40,
                cols: 110,
            },
        ),
        (
            format!("{row}\npane\n{row}"),
            Viewport {
                x: 0,
                y: 2,
                rows: 36,
                cols: 120,
            },
        ),
    ] {
        let layout = format!(
            "layout {{ {body}\nfloating_panes {{ pane x=\"25%\" y=\"25%\" width=\"50%\" height=\"50%\"; }}; }}"
        );
        let plugins = vec![
            ("yinz:compact-bar", 100),
            ("yinz:status-bar", 101),
            ("yinz:compact-bar", 102),
            ("yinz:status-bar", 103),
        ];
        assert_eq!(
            viewport_after_applying_layout_with_plugins(
                &layout,
                vec![(TerminalId::new(1), None)],
                plugins,
                Size {
                    cols: 120,
                    rows: 40
                }
            ),
            expected,
            "{layout}"
        );
    }
}

#[test]
fn incomplete_ui_rows_and_all_plugin_layouts_retain_usable_viewports() {
    for (body, terminals, expected) in [
        (
            r#"pane size=2 split_direction="vertical" {
            pane borderless=true { plugin location="yinz:compact-bar"; }
            pane borderless=true
        }
        pane"#,
            2,
            Viewport {
                x: 0,
                y: 0,
                rows: 40,
                cols: 120,
            },
        ),
        (
            r#"pane split_direction="vertical" {
            pane borderless=true { plugin location="yinz:compact-bar"; }
            pane borderless=true { plugin location="yinz:status-bar"; }
        }"#,
            0,
            Viewport {
                x: 60,
                y: 0,
                rows: 40,
                cols: 60,
            },
        ),
        (
            r#"pane borderless=true { plugin location="yinz:compact-bar"; }"#,
            0,
            Viewport {
                x: 0,
                y: 0,
                rows: 40,
                cols: 120,
            },
        ),
    ] {
        let layout = format!(
            "layout {{ {body}\nfloating_panes {{ pane x=\"25%\" y=\"25%\" width=\"50%\" height=\"50%\"; }}; }}"
        );
        assert_eq!(
            viewport_after_applying_layout_with_plugins(
                &layout,
                (1..=terminals)
                    .map(|id| (TerminalId::new(id), None))
                    .collect(),
                vec![("yinz:compact-bar", 100), ("yinz:status-bar", 101)],
                Size {
                    cols: 120,
                    rows: 40
                }
            ),
            expected,
            "{layout}"
        );
    }
}

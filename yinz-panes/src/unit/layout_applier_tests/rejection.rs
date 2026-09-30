use super::*;

#[test]
fn rejected_layouts_leave_existing_panes_and_processes_untouched() {
    for override_existing in [false, true] {
        for (rejected, retain, small) in [
            (r#"layout { pane size=200; pane; }"#, false, false),
            (
                r#"layout { pane command="replacement"; pane { plugin location="yinz:missing"; }; }"#,
                false,
                false,
            ),
            (
                r#"layout { pane command="replacement"; floating_panes { pane { plugin location="yinz:missing"; }; }; }"#,
                false,
                false,
            ),
            (
                r#"layout { pane command="replacement"; floating_panes { pane; }; }"#,
                false,
                true,
            ),
            (r#"layout { pane command="replacement"; }"#, true, true),
        ] {
            if retain && !override_existing {
                continue;
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
                pane_frame_style,
                mut focus_pane_id,
                os_api,
                debug,
                arrow_fonts,
                styled_underlines,
                osc8_hyperlinks,
                explicitly_disable_kitty_keyboard_protocol,
                pty_receiver,
                plugin_receiver,
            ) = create_layout_applier_fixtures_with_receivers(if small {
                Size { cols: 5, rows: 5 }
            } else {
                Size {
                    cols: 100,
                    rows: 50,
                }
            });
            let kitty_image_store = Rc::new(RefCell::new(KittyImageStore::default()));
            macro_rules! applier {
                () => {
                    LayoutApplier::new(
                        &viewport,
                        &senders,
                        &sixel_image_store,
                        &kitty_image_store,
                        &link_handler,
                        &terminal_emulator_colors,
                        &terminal_emulator_color_codes,
                        &character_cell_size,
                        &connected_clients,
                        &style,
                        &display_area,
                        &mut tiled_panes,
                        &mut floating_panes,
                        pane_frame_style,
                        &mut focus_pane_id,
                        os_api.as_ref(),
                        debug,
                        arrow_fonts,
                        styled_underlines,
                        osc8_hyperlinks,
                        explicitly_disable_kitty_keyboard_protocol,
                        None,
                    )
                };
            }
            let initial = if small {
                r#"layout { pane command="original" name="keep me"; }"#
            } else {
                r#"layout { pane command="original" name="keep me"; floating_panes { pane command="original-float"; }; }"#
            };
            let (layout, floating) = parse_kdl_layout(initial);
            applier!()
                .apply_layout(
                    layout,
                    floating,
                    vec![(TerminalId::new(1), None)],
                    vec![(TerminalId::new(2), None)],
                    HashMap::new(),
                    ClientId::new(1),
                )
                .unwrap();
            let before = take_pane_state_snapshot(
                &tiled_panes,
                &floating_panes,
                &focus_pane_id,
                &viewport,
                &display_area,
            );
            collect_close_pane_messages(&pty_receiver);
            collect_unload_plugin_messages(&plugin_receiver);

            let (layout, floating) = parse_kdl_layout(rejected);
            let result = if override_existing {
                applier!().override_layout(
                    layout,
                    floating,
                    vec![(TerminalId::new(3), None)],
                    vec![(TerminalId::new(4), None), (TerminalId::new(5), None)],
                    HashMap::new(),
                    retain,
                    retain,
                    ClientId::new(1),
                )
            } else {
                applier!().apply_layout(
                    layout,
                    floating,
                    vec![(TerminalId::new(3), None)],
                    vec![(TerminalId::new(4), None), (TerminalId::new(5), None)],
                    HashMap::new(),
                    ClientId::new(1),
                )
            };
            assert!(result.is_err(), "accepted invalid layout: {rejected}");
            if override_existing {
                let (layout, floating) = parse_kdl_layout(rejected);
                let result = if floating.is_empty() {
                    applier!().override_tiled_panes_layout_for_existing_panes(
                        &layout,
                        vec![(TerminalId::new(3), None)],
                        &mut HashMap::new(),
                        retain,
                        retain,
                        ClientId::new(1),
                    )
                } else {
                    applier!()
                        .override_floating_panes_layout_for_existing_panes(
                            &floating,
                            vec![(TerminalId::new(4), None), (TerminalId::new(5), None)],
                            &mut HashMap::new(),
                            retain,
                            retain,
                        )
                        .map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "direct override accepted invalid layout: {rejected}"
                );
            }

            assert_eq!(
                before,
                take_pane_state_snapshot(
                    &tiled_panes,
                    &floating_panes,
                    &focus_pane_id,
                    &viewport,
                    &display_area
                )
            );
            assert!(collect_close_pane_messages(&pty_receiver).is_empty());
            assert!(collect_unload_plugin_messages(&plugin_receiver).is_empty());
        }
    }
}
